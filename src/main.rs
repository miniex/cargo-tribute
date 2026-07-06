//! From a Cargo dependency tree, write a REUSE-style LICENSES/ folder (one canonical
//! text per license used), a NOTICES/ folder (NOTICE files deps ship), and a per-crate
//! attribution manifest with copyright lines. `--check` verifies the output is current
//! and every license is accepted, without writing anything.

use cargo_metadata::camino::Utf8Path;
use cargo_metadata::semver::{Version, VersionReq};
use cargo_metadata::{DependencyKind, MetadataCommand, Package, PackageId};
use serde::{Deserialize, Serialize};
use spdx::expression::{ExprNode, Operator};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

// default allowed licenses; also the OR preference order (earlier wins when an
// "A OR B" can pick either). Overridable via tribute.toml.
const DEFAULT_ACCEPTED: &[&str] =
    &["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "0BSD", "Zlib", "Unlicense", "Unicode-3.0"];

// canonical text for a license or exception id, from the spdx crate's bundled corpus
// (the `text` feature). covers every SPDX id, so no texts are hand-maintained here.
fn canonical_text(id: &str) -> Option<&'static str> {
    spdx::license_id(id).map(|l| l.text()).or_else(|| spdx::exception_id(id).map(|e| e.text()))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Config {
    accepted: Option<Vec<String>>,
    manifest: Option<String>,
    #[serde(rename = "licenses-dir")]
    licenses_dir: Option<String>,
    #[serde(rename = "notices-dir")]
    notices_dir: Option<String>,
    clarify: Option<Vec<Clarify>>,
}

// override a crate's license when its `license` field is missing (crates that use
// `license-file` instead), wrong, or non-SPDX. `version` optional: omit to match any.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Clarify {
    name: String,
    version: Option<String>,
    expression: String,
}

struct Settings {
    accepted: Vec<String>,
    clarify: Vec<Clarify>,
    manifest: PathBuf,     // absolute output path
    manifest_link: String, // relative name, for messages
    licenses_dir: PathBuf, // absolute output dir
    licenses_link: String, // relative name, for markdown links + messages
    notices_dir: PathBuf,  // absolute output dir for NOTICE files
    notices_link: String,  // relative name, for markdown links + messages
}

// anchor tribute.toml and outputs to the workspace root, not the cwd, so
// --manifest-path against a crate elsewhere reads and writes beside that crate.
fn load_settings(root: &Utf8Path) -> Result<Settings, String> {
    let path = root.join("tribute.toml");
    // only a missing file falls back to defaults; a present-but-unreadable config
    // (bad permissions, non-UTF-8) must error, not silently ignore the policy.
    let cfg: Config = match fs::read_to_string(&path) {
        Ok(s) => toml::from_str(&s).map_err(|e| format!("tribute.toml: {e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(e) => return Err(format!("{path}: {e}")),
    };
    let manifest_link = cfg.manifest.unwrap_or_else(|| "THIRD-PARTY.md".into());
    let licenses_link = cfg.licenses_dir.unwrap_or_else(|| "LICENSES".into());
    let notices_link = cfg.notices_dir.unwrap_or_else(|| "NOTICES".into());
    // keep outputs inside the project: an absolute or `..` path would let the write and the
    // orphan-cleanup (which deletes `.txt`) touch files outside the tree.
    relative_inside("manifest", &manifest_link)?;
    relative_inside("licenses-dir", &licenses_link)?;
    relative_inside("notices-dir", &notices_link)?;
    Ok(Settings {
        accepted: cfg.accepted.unwrap_or_else(|| DEFAULT_ACCEPTED.iter().map(|s| s.to_string()).collect()),
        clarify: cfg.clarify.unwrap_or_default(),
        manifest: root.join(&manifest_link).into(),
        licenses_dir: root.join(&licenses_link).into(),
        notices_dir: root.join(&notices_link).into(),
        manifest_link,
        licenses_link,
        notices_link,
    })
}

// reject a config output path that is absolute, escapes the project via `..`, or names
// no real target (empty or "."). the last would resolve to the project root itself, so
// orphan-cleanup (which deletes bundled-id `.txt`s) would then scan the whole tree.
fn relative_inside(field: &str, link: &str) -> Result<(), String> {
    use std::path::Component;
    let p = Path::new(link);
    let escapes = p.is_absolute() || p.components().any(|c| c == Component::ParentDir);
    let has_target = p.components().any(|c| matches!(c, Component::Normal(_)));
    if escapes || !has_target {
        return Err(format!("tribute.toml: {field} must be a relative path inside the project (got '{link}')"));
    }
    Ok(())
}

// wrap an io result with the path, so a failure names the file instead of a bare errno.
fn io<T>(path: &Path, r: std::io::Result<T>) -> Result<T, String> {
    r.map_err(|e| format!("{}: {e}", path.display()))
}

// per-crate extras from the crate source: copyright lines out of license/notice
// files (authors as a fallback), and the NOTICE body Apache-2.0 4(d) says to pass along.
struct Extras {
    copyrights: Vec<String>, // "Copyright ..." lines, deduped and sorted
    authors: Vec<String>,    // metadata authors, as declared
    notice: Option<String>,  // NOTICE file contents, LF-normalized
}

// scan the crate root (already local in cargo's registry cache, so still offline).
// best-effort: an unreadable or huge file is skipped, extras never gate.
fn harvest_extras(pkg: &Package) -> Extras {
    const MAX_LEN: u64 = 1_000_000;
    let mut copyrights = BTreeSet::new();
    let mut notice_parts: Vec<String> = Vec::new();
    if let Some(dir) = pkg.manifest_path.parent() {
        let mut names: Vec<String> = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort(); // deterministic order across platforms
        for name in names {
            let lower = name.to_lowercase();
            let stem = lower.split('.').next().unwrap_or("");
            let is_notice = stem == "notice" || stem == "notices";
            let is_license = ["license", "licence", "copying", "copyright"].iter().any(|p| lower.starts_with(p));
            if !is_notice && !is_license {
                continue;
            }
            let path = dir.join(&name);
            // skip directories (e.g. a LICENSES/ folder) and oversized files.
            if !fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= MAX_LEN) {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else { continue };
            // LF-normalize now, so a CRLF NOTICE source can't read as stale in --check.
            let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
            copyright_lines(&text, &mut copyrights);
            if is_notice {
                notice_parts.push(text);
            }
        }
    }
    Extras {
        copyrights: copyrights.into_iter().collect(),
        authors: pkg.authors.clone(),
        notice: (!notice_parts.is_empty()).then(|| notice_parts.join("\n")),
    }
}

// "Copyright ..." lines from a license or notice file, whitespace-normalized.
fn copyright_lines(text: &str, out: &mut BTreeSet<String>) {
    // fill-in-the-blank template lines (the Apache-2.0 appendix, MIT boilerplate)
    // name nobody.
    const PLACEHOLDERS: &[&str] = &[
        "yyyy", // [yyyy] and {yyyy}
        "name of copyright owner",
        "<year>",
        "<owner>",
        "<copyright holder", // covers <copyright holder> and <copyright holders>
        "<name of author>",
        "{year}",
        "{owner}",
    ];
    // words after "Copyright" in headings and license prose ("COPYRIGHT AND
    // PERMISSION NOTICE"), never in a real holder statement.
    const NOT_A_HOLDER: &[&str] = &["and", "notice", "notices", "license", "licenses", "law", "laws"];
    for raw in text.lines() {
        let line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        // only a capitalized "Copyright" (or the sign) starts a real statement; a
        // lower-case "copyright ..." is a wrapped mid-sentence fragment of the
        // license body itself (common in the Apache-2.0 text).
        let Some(rest) = ["Copyright", "COPYRIGHT", "©"].iter().find_map(|p| line.strip_prefix(p)) else { continue };
        if PLACEHOLDERS.iter().any(|p| line.to_lowercase().contains(p)) {
            continue;
        }
        // a bare "Copyright" heading or "Copyright (c)" alone names nobody.
        let holder = rest.replace("(c)", " ").replace("(C)", " ");
        if !holder.chars().any(|c| c.is_alphanumeric()) {
            continue;
        }
        let first = holder.split_whitespace().next().unwrap_or("").trim_matches(|c: char| !c.is_alphanumeric());
        if NOT_A_HOLDER.contains(&first.to_lowercase().as_str()) {
            continue;
        }
        out.insert(line);
    }
}

// "Alice <alice@example.com>" -> "Alice"; an email-only entry is kept as-is.
fn display_author(a: &str) -> &str {
    match a.split_once('<') {
        Some((name, _)) if !name.trim().is_empty() => name.trim(),
        _ => a.trim(),
    }
}

// a LICENSES/<id>.txt cargo-tribute could write (stem is an SPDX license or exception id)
// that is no longer used. a .txt whose stem is not an SPDX id is hand-added and left alone.
fn is_stale_license(path: &Path, texts: &BTreeMap<&str, &'static str>) -> bool {
    path.extension().is_some_and(|x| x == "txt")
        && path
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|s| canonical_text(s).is_some() && !texts.contains_key(s))
}

// a NOTICES/<name>-<version>.txt cargo-tribute could write that is no longer used.
// only a stem ending in "-<semver>" is ours; anything else is hand-added and left alone.
fn is_stale_notice(path: &Path, notices: &BTreeMap<String, String>) -> bool {
    path.extension().is_some_and(|x| x == "txt")
        && path.file_stem().and_then(|s| s.to_str()).is_some_and(|stem| {
            stem.rsplit_once('-').is_some_and(|(_, v)| Version::parse(v).is_ok()) && !notices.contains_key(stem)
        })
}

const HELP: &str = "\
cargo-tribute -- REUSE-style third-party license attribution from a Cargo tree

USAGE:
    cargo tribute [OPTIONS]

OPTIONS:
        --check              verify the output is current; do not write (exit 1 if stale)
        --manifest-path <P>  path to Cargo.toml (default: auto-detect from the cwd)
        --locked             forwarded to `cargo metadata` (also --offline, --frozen)
        --features <F>       forwarded to `cargo metadata`, to attribute feature-gated
                             deps (also --all-features, --no-default-features,
                             --filter-platform <T>)
        --json               print the resolved attribution as JSON instead of a summary
    -h, --help               print this help
    -V, --version            print version

CONFIG (tribute.toml in the project root, all optional):
    accepted = [\"MIT\", \"Apache-2.0\", ...]   # allowed licenses; also the OR preference order
    manifest = \"THIRD-PARTY.md\"              # attribution manifest path
    licenses-dir = \"LICENSES\"                # folder for the canonical license texts
    notices-dir = \"NOTICES\"                  # folder for NOTICE files shipped by dependencies

    [[clarify]]                              # override a crate's license (missing/wrong/non-SPDX)
    name = \"ring\"
    version = \"0.17.8\"                       # optional semver req; omit to match any version
    expression = \"MIT AND ISC AND OpenSSL\"
";

fn main() -> ExitCode {
    let mut check = false;
    let mut json = false;
    let mut manifest_path = None;
    // flags forwarded verbatim to `cargo metadata`, e.g. --locked/--offline/--frozen
    // so a CI --check resolves deterministically and offline.
    let mut cargo_flags: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().map(String::as_str) == Some("tribute") {
        args.next(); // cargo passes the subcommand name when invoked as `cargo tribute`
    }
    while let Some(a) = args.next() {
        match a.as_str() {
            "--check" => check = true,
            "--json" => json = true,
            "--manifest-path" => match args.next() {
                Some(p) => manifest_path = Some(p),
                None => {
                    eprintln!("cargo-tribute: --manifest-path needs a value");
                    return ExitCode::FAILURE;
                }
            },
            "--locked" | "--offline" | "--frozen" | "--all-features" | "--no-default-features" => cargo_flags.push(a),
            // value-taking passthroughs; forward the flag and its value verbatim.
            "--features" | "--filter-platform" => match args.next() {
                Some(v) => cargo_flags.extend([a, v]),
                None => {
                    eprintln!("cargo-tribute: {a} needs a value");
                    return ExitCode::FAILURE;
                }
            },
            "-h" | "--help" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("cargo-tribute {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            // accept the `--features=foo`/`--filter-platform=foo` spellings cargo also takes.
            _ if a.starts_with("--features=") || a.starts_with("--filter-platform=") => cargo_flags.push(a),
            other => {
                eprintln!("cargo-tribute: unknown argument '{other}' (try --help)");
                return ExitCode::FAILURE;
            }
        }
    }
    match run(check, json, manifest_path, cargo_flags) {
        Ok(msg) => {
            println!("{msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("cargo-tribute: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(check: bool, json: bool, manifest_path: Option<String>, cargo_flags: Vec<String>) -> Result<String, String> {
    let mut cmd = MetadataCommand::new();
    if let Some(p) = manifest_path {
        cmd.manifest_path(PathBuf::from(p));
    }
    if !cargo_flags.is_empty() {
        cmd.other_options(cargo_flags);
    }
    let meta = cmd.exec().map_err(|e| e.to_string())?;
    let set = load_settings(&meta.workspace_root)?;
    // a typo in `accepted` (e.g. "Apache2.0") would silently reject that license; flag it.
    for a in &set.accepted {
        if spdx::license_id(a).is_none() {
            eprintln!("cargo-tribute: warning: accepted license '{a}' is not a known SPDX id");
        }
    }
    let resolve = meta.resolve.as_ref().ok_or("no dependency resolution (need a Cargo.toml)")?;

    let node_of: BTreeMap<&PackageId, _> = resolve.nodes.iter().map(|n| (&n.id, n)).collect();
    let pkg_of: BTreeMap<&PackageId, &Package> = meta.packages.iter().map(|p| (&p.id, p)).collect();
    let workspace: BTreeSet<&PackageId> = meta.workspace_members.iter().collect();

    // normal-dependency closure of the workspace members, minus the members themselves.
    let mut seen = BTreeSet::new();
    let mut stack: Vec<&PackageId> = meta.workspace_members.iter().collect();
    let mut deps = BTreeSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(node) = node_of.get(id) else { continue };
        for dep in &node.deps {
            if !dep.dep_kinds.iter().any(|k| k.kind == DependencyKind::Normal) {
                continue;
            }
            if !workspace.contains(&dep.pkg) {
                deps.insert(&dep.pkg);
            }
            stack.push(&dep.pkg);
        }
    }

    // choose a license per dependency; collect crates grouped by license.
    // effective: expression actually used per crate (clarified or declared), so the
    // manifest reports that, not the crate's possibly-wrong license field.
    let mut by_license: BTreeMap<String, Vec<&Package>> = BTreeMap::new();
    let mut effective: BTreeMap<&PackageId, &str> = BTreeMap::new();
    let mut chosen_of: BTreeMap<&PackageId, BTreeSet<String>> = BTreeMap::new();
    let mut used_exceptions: BTreeSet<String> = BTreeSet::new();
    let mut failures = Vec::new();
    for id in &deps {
        // every resolve-graph id is also in meta.packages; guard the lookup so a
        // cargo_metadata invariant break surfaces as an error, not an index panic.
        let Some(&pkg) = pkg_of.get(id) else {
            failures.push(format!("{id:?}: no package metadata (internal)"));
            continue;
        };
        let name = format!("{} {}", pkg.name, pkg.version);
        let clarified = clarify_expr(&set.clarify, pkg.name.as_ref(), &pkg.version);
        let Some(expr_str) = clarified.or(pkg.license.as_deref()) else {
            failures.push(format!("{name}: no license field (add a [[clarify]] entry to tribute.toml)"));
            continue;
        };
        effective.insert(*id, expr_str);
        // LAX accepts the legacy `/` OR-separator and lower-case operators still
        // found in older crates (e.g. "MIT/Apache-2.0", "Unlicense/MIT").
        let expr = match spdx::Expression::parse_mode(expr_str, spdx::ParseMode::LAX) {
            Ok(e) => e,
            Err(e) => {
                failures.push(format!("{name}: unparsable SPDX '{expr_str}' ({e})"));
                continue;
            }
        };
        match choose(&expr, &set.accepted) {
            Some(chosen) => {
                // a WITH exception on a chosen license contributes its own text file.
                for ex in exceptions_for(&expr, &chosen) {
                    used_exceptions.insert(ex);
                }
                for lic in &chosen {
                    by_license.entry(lic.clone()).or_default().push(pkg);
                }
                chosen_of.insert(*id, chosen);
            }
            None => failures.push(format!("{name}: license '{expr_str}' not in the accepted set")),
        }
    }
    // warn on clarify entries that matched no dependency, so a typo in name or version
    // is visible instead of silently ignored.
    for c in &set.clarify {
        let matched =
            deps.iter().any(|id| pkg_of.get(id).is_some_and(|p| clarify_matches(c, p.name.as_ref(), &p.version)));
        if !matched {
            let ver = c.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default();
            eprintln!("cargo-tribute: warning: clarify for '{}{ver}' matched no dependency", c.name);
        }
    }

    if !failures.is_empty() {
        return Err(format!("license policy failed:\n  {}", failures.join("\n  ")));
    }

    // per-crate copyright lines and NOTICE bodies, from the local sources.
    let mut extras: BTreeMap<&PackageId, Extras> = BTreeMap::new();
    let mut notices: BTreeMap<String, String> = BTreeMap::new();
    for id in &deps {
        let Some(&pkg) = pkg_of.get(id) else { continue };
        let x = harvest_extras(pkg);
        if let Some(n) = &x.notice {
            notices.insert(format!("{}-{}", pkg.name, pkg.version), n.clone());
        }
        extras.insert(*id, x);
    }

    // resolve each used license and exception id to its canonical text.
    let mut texts: BTreeMap<&str, &'static str> = BTreeMap::new();
    for id in by_license.keys().map(String::as_str).chain(used_exceptions.iter().map(String::as_str)) {
        let text = canonical_text(id).ok_or_else(|| format!("no canonical text for SPDX id '{id}'"))?;
        texts.insert(id, text);
    }
    // --json is a read-only report of what the tree resolves to; it never writes or checks.
    if json {
        return render_json(&deps, &pkg_of, &effective, &chosen_of, &by_license, &used_exceptions, &extras);
    }
    let manifest = render_manifest(&by_license, &effective, &extras, &set.licenses_link, &set.notices_link);

    if check {
        let stale = stale_outputs(&set.licenses_dir, &texts, &set.notices_dir, &notices, &set.manifest, &manifest);
        if !stale.is_empty() {
            return Err(format!("out of date (run `cargo tribute`):\n  {}", stale.join("\n  ")));
        }
        let n = if notices.is_empty() { String::new() } else { format!(", {} notices", notices.len()) };
        Ok(format!("up to date: {} license texts{n}, {} crates", texts.len(), deps.len()))
    } else {
        io(&set.licenses_dir, fs::create_dir_all(&set.licenses_dir))?;
        // drop license/exception texts cargo-tribute wrote that are no longer used; leave other files
        if let Ok(entries) = fs::read_dir(&set.licenses_dir) {
            for e in entries.flatten() {
                let p = e.path();
                if is_stale_license(&p, &texts) {
                    let _ = fs::remove_file(p);
                }
            }
        }
        for (id, text) in &texts {
            let p = set.licenses_dir.join(format!("{id}.txt"));
            io(&p, fs::write(&p, text))?;
        }
        // drop NOTICE files we wrote that are no longer used; keep the folder only
        // while there is something to ship.
        if let Ok(entries) = fs::read_dir(&set.notices_dir) {
            for e in entries.flatten() {
                let p = e.path();
                if is_stale_notice(&p, &notices) {
                    let _ = fs::remove_file(p);
                }
            }
        }
        if notices.is_empty() {
            let _ = fs::remove_dir(&set.notices_dir); // only removes an empty dir
        } else {
            io(&set.notices_dir, fs::create_dir_all(&set.notices_dir))?;
            for (stem, text) in &notices {
                let p = set.notices_dir.join(format!("{stem}.txt"));
                io(&p, fs::write(&p, text))?;
            }
        }
        // manifest path is configurable and may sit in a subdir; create it like licenses_dir
        if let Some(parent) = set.manifest.parent() {
            io(parent, fs::create_dir_all(parent))?;
        }
        io(&set.manifest, fs::write(&set.manifest, &manifest))?;
        let n = if notices.is_empty() {
            String::new()
        } else {
            format!(", {}/ ({} notices)", set.notices_link, notices.len())
        };
        Ok(format!(
            "wrote {}/ ({} license texts){n} and {} ({} crates)",
            set.licenses_link,
            texts.len(),
            set.manifest_link,
            deps.len()
        ))
    }
}

// a clarify entry applies to this crate: name equal, and if the entry gives a version it
// parses as a semver requirement the crate satisfies (so "1.0" matches 1.0.0, like Cargo).
fn clarify_matches(c: &Clarify, name: &str, version: &Version) -> bool {
    c.name == name && c.version.as_deref().is_none_or(|v| VersionReq::parse(v).is_ok_and(|req| req.matches(version)))
}

// a tribute.toml [[clarify]] expression overriding this crate's declared license.
fn clarify_expr<'a>(clarify: &'a [Clarify], name: &str, version: &Version) -> Option<&'a str> {
    clarify.iter().find(|c| clarify_matches(c, name, version)).map(|c| c.expression.as_str())
}

// SPDX exception ids (from `A WITH exception`) attached to a license we actually chose, so
// their text ships too. a WITH on a license the OR-pick dropped is not attributed.
fn exceptions_for(expr: &spdx::Expression, chosen: &BTreeSet<String>) -> Vec<String> {
    expr.iter()
        .filter_map(|node| match node {
            ExprNode::Req(r) => {
                let ex = r.req.addition.as_ref()?.id()?;
                let lic = r.req.license.id()?.name;
                chosen.contains(lic).then(|| ex.name.to_string())
            }
            ExprNode::Op(_) => None,
        })
        .collect()
}

// walk the SPDX expression (postfix) to the licenses we attribute, or None if the
// accepted set can't cover it. OR keeps the preferred operand, AND unions both, an
// unaccepted leaf is None.
fn choose(expr: &spdx::Expression, accepted: &[String]) -> Option<BTreeSet<String>> {
    let mut stack: Vec<Option<BTreeSet<String>>> = Vec::new();
    for node in expr.iter() {
        match node {
            ExprNode::Req(req) => {
                let leaf =
                    req.req.license.id().map(|id| id.name).filter(|n| accepted.iter().any(|a| a == n)).map(|n| {
                        let mut s = BTreeSet::new();
                        s.insert(n.to_string());
                        s
                    });
                stack.push(leaf);
            }
            ExprNode::Op(op) => {
                let b = stack.pop()?;
                let a = stack.pop()?;
                stack.push(combine(*op, a, b, accepted));
            }
        }
    }
    stack.pop().flatten()
}

fn combine(
    op: Operator,
    a: Option<BTreeSet<String>>,
    b: Option<BTreeSet<String>>,
    accepted: &[String],
) -> Option<BTreeSet<String>> {
    match op {
        Operator::And => match (a, b) {
            (Some(mut x), Some(y)) => {
                x.extend(y);
                Some(x)
            }
            _ => None,
        },
        Operator::Or => match (a, b) {
            (Some(x), Some(y)) => Some(if best(&x, accepted) <= best(&y, accepted) { x } else { y }),
            (Some(x), None) | (None, Some(x)) => Some(x),
            (None, None) => None,
        },
    }
}

fn best(set: &BTreeSet<String>, accepted: &[String]) -> usize {
    set.iter().map(|l| accepted.iter().position(|p| p == l).unwrap_or(usize::MAX)).min().unwrap_or(usize::MAX)
}

#[derive(Serialize)]
struct Report<'a> {
    licenses: Vec<&'a str>,   // license ids used, with a text in the LICENSES dir
    exceptions: Vec<&'a str>, // WITH-exception ids used
    crates: Vec<CrateEntry<'a>>,
}

#[derive(Serialize)]
struct CrateEntry<'a> {
    name: &'a str,
    version: String,
    expression: &'a str,      // effective SPDX (clarified or declared)
    licenses: Vec<&'a str>,   // ids this crate is attributed under
    authors: &'a [String],    // metadata authors, as declared
    copyrights: &'a [String], // "Copyright ..." lines from license/notice files
    notice: Option<&'a str>,  // NOTICE body, when the crate ships one
}

// the resolved attribution as JSON, for audit/pipeline use. read-only: no files touched.
fn render_json(
    deps: &BTreeSet<&PackageId>,
    pkg_of: &BTreeMap<&PackageId, &Package>,
    effective: &BTreeMap<&PackageId, &str>,
    chosen_of: &BTreeMap<&PackageId, BTreeSet<String>>,
    by_license: &BTreeMap<String, Vec<&Package>>,
    used_exceptions: &BTreeSet<String>,
    extras: &BTreeMap<&PackageId, Extras>,
) -> Result<String, String> {
    let crates: Vec<CrateEntry> = deps
        .iter()
        .filter_map(|id| {
            let pkg = pkg_of.get(id).copied()?;
            let chosen = chosen_of.get(*id)?;
            let x = extras.get(*id);
            Some(CrateEntry {
                name: pkg.name.as_ref(),
                version: pkg.version.to_string(),
                expression: effective.get(*id).copied().unwrap_or(""),
                licenses: chosen.iter().map(String::as_str).collect(),
                authors: x.map(|x| x.authors.as_slice()).unwrap_or(&[]),
                copyrights: x.map(|x| x.copyrights.as_slice()).unwrap_or(&[]),
                notice: x.and_then(|x| x.notice.as_deref()),
            })
        })
        .collect();
    let report = Report {
        licenses: by_license.keys().map(String::as_str).collect(),
        exceptions: used_exceptions.iter().map(String::as_str).collect(),
        crates,
    };
    serde_json::to_string_pretty(&report).map_err(|e| e.to_string())
}

// disk content equals `want`, ignoring line-ending style. output is always written LF,
// so `want` is LF; a CRLF checkout (git autocrlf) of it must not read as stale. strip
// CR from disk before comparing.
fn matches_output(disk: Option<String>, want: &str) -> bool {
    disk.is_some_and(|d| d.replace("\r\n", "\n") == want)
}

// paths a plain run would create, change, or delete; empty means --check passes.
// includes orphaned license/notice files the write path removes, so --check cannot
// pass while stale files still sit in the tree.
fn stale_outputs(
    licenses_dir: &Path,
    texts: &BTreeMap<&str, &'static str>,
    notices_dir: &Path,
    notices: &BTreeMap<String, String>,
    manifest_path: &Path,
    manifest: &str,
) -> Vec<String> {
    let mut stale = Vec::new();
    for (id, want) in texts {
        let path = licenses_dir.join(format!("{id}.txt"));
        if !matches_output(fs::read_to_string(&path).ok(), want) {
            stale.push(path.display().to_string());
        }
    }
    if let Ok(entries) = fs::read_dir(licenses_dir) {
        for e in entries.flatten() {
            let p = e.path();
            if is_stale_license(&p, texts) {
                stale.push(p.display().to_string());
            }
        }
    }
    for (stem, want) in notices {
        let path = notices_dir.join(format!("{stem}.txt"));
        if !matches_output(fs::read_to_string(&path).ok(), want) {
            stale.push(path.display().to_string());
        }
    }
    if let Ok(entries) = fs::read_dir(notices_dir) {
        for e in entries.flatten() {
            let p = e.path();
            if is_stale_notice(&p, notices) {
                stale.push(p.display().to_string());
            }
        }
    }
    if !matches_output(fs::read_to_string(manifest_path).ok(), manifest) {
        stale.push(manifest_path.display().to_string());
    }
    stale
}

fn render_manifest(
    by_license: &BTreeMap<String, Vec<&Package>>,
    effective: &BTreeMap<&PackageId, &str>,
    extras: &BTreeMap<&PackageId, Extras>,
    licenses_dir: &str,
    notices_dir: &str,
) -> String {
    let mut out = String::from(
        "# Third-party licenses\n\nDependencies linked into this crate, grouped by license; full texts are in \
         [`",
    );
    out.push_str(licenses_dir);
    out.push_str("/`](");
    out.push_str(licenses_dir);
    out.push(')');
    // mention the notices folder only when this tree ships one.
    if extras.values().any(|x| x.notice.is_some()) {
        out.push_str(&format!(", NOTICE files shipped by dependencies in [`{notices_dir}/`]({notices_dir})"));
    }
    out.push_str(". Generated by `cargo tribute`; do not edit.\n\n");
    for (id, pkgs) in by_license {
        let mut ps: Vec<&Package> = pkgs.clone();
        ps.sort_by(|a, b| (&*a.name, &a.version).cmp(&(&*b.name, &b.version)));
        ps.dedup_by(|a, b| a.id == b.id);
        out.push_str(&format!("## {id}\n\nText: [`{licenses_dir}/{id}.txt`]({licenses_dir}/{id}.txt)\n\n"));
        for p in ps {
            let url = p.repository.clone().unwrap_or_else(|| format!("https://crates.io/crates/{}", p.name));
            out.push_str(&format!("- [{} {}]({url})", p.name, p.version));
            // show the effective SPDX (clarified or declared) when it differs from the
            // section license, so WITH exceptions and dual-license picks are not hidden by
            // the grouping. the exception's own text is written to the licenses dir too.
            if let Some(expr) = effective.get(&p.id).copied().filter(|e| *e != id.as_str()) {
                out.push_str(&format!(" -- `{expr}`"));
            }
            if let Some(x) = extras.get(&p.id) {
                // the crate's holders; MIT/BSD want the copyright notice itself reproduced.
                if !x.copyrights.is_empty() {
                    out.push_str(&format!(" -- {}", x.copyrights.join("; ")));
                } else {
                    let names: Vec<&str> =
                        x.authors.iter().map(|a| display_author(a)).filter(|s| !s.is_empty()).collect();
                    if !names.is_empty() {
                        out.push_str(&format!(" -- by {}", names.join(", ")));
                    }
                }
                if x.notice.is_some() {
                    out.push_str(&format!(
                        " -- [NOTICE]({notices_dir}/{name}-{ver}.txt)",
                        name = p.name,
                        ver = p.version
                    ));
                }
            }
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(s: &str) -> Option<Vec<String>> {
        let accepted: Vec<String> = DEFAULT_ACCEPTED.iter().map(|s| s.to_string()).collect();
        let e = spdx::Expression::parse_mode(s, spdx::ParseMode::LAX).unwrap();
        choose(&e, &accepted).map(|set| set.into_iter().collect())
    }

    #[test]
    fn or_picks_preferred() {
        assert_eq!(pick("MIT OR Apache-2.0"), Some(vec!["MIT".into()]));
        assert_eq!(pick("Apache-2.0 OR MIT"), Some(vec!["MIT".into()]));
        assert_eq!(pick("Zlib OR Apache-2.0 OR MIT"), Some(vec!["MIT".into()]));
    }

    #[test]
    fn and_unions_both() {
        assert_eq!(pick("(MIT OR Apache-2.0) AND Unicode-3.0"), Some(vec!["MIT".into(), "Unicode-3.0".into()]));
    }

    #[test]
    fn legacy_slash_is_or() {
        assert_eq!(pick("MIT/Apache-2.0"), Some(vec!["MIT".into()]));
        assert_eq!(pick("Unlicense/MIT"), Some(vec!["MIT".into()]));
    }

    #[test]
    fn rejects_unaccepted() {
        assert_eq!(pick("GPL-3.0-only"), None);
        assert_eq!(pick("MIT AND GPL-3.0-only"), None);
    }

    #[test]
    fn clarify_matches_name_and_version() {
        let v = |s: &str| -> Version { s.parse().unwrap() };
        let c = vec![
            Clarify { name: "ring".into(), version: None, expression: "MIT AND ISC".into() },
            Clarify { name: "foo".into(), version: Some("1.0".into()), expression: "BSD-3-Clause".into() },
        ];
        assert_eq!(clarify_expr(&c, "ring", &v("0.17.8")), Some("MIT AND ISC")); // omitted version matches any
        assert_eq!(clarify_expr(&c, "foo", &v("1.0.0")), Some("BSD-3-Clause")); // req "1.0" matches 1.0.0
        assert_eq!(clarify_expr(&c, "foo", &v("1.4.0")), Some("BSD-3-Clause")); // and 1.4.0 (caret req)
        assert_eq!(clarify_expr(&c, "foo", &v("2.0.0")), None); // out of req range
        assert_eq!(clarify_expr(&c, "bar", &v("1.0.0")), None); // name mismatch
    }

    #[test]
    fn stale_detects_missing_and_orphan() {
        let dir = std::env::temp_dir().join(format!("tribute-test-{}", std::process::id()));
        let lic = dir.join("LICENSES");
        let not = dir.join("NOTICES");
        fs::create_dir_all(&lic).unwrap();
        fs::create_dir_all(&not).unwrap();
        let manifest_path = dir.join("THIRD-PARTY.md");
        let mut texts: BTreeMap<&str, &'static str> = BTreeMap::new();
        texts.insert("MIT", "MIT TEXT");
        let mut notices: BTreeMap<String, String> = BTreeMap::new();
        notices.insert("dep-1.0.0".into(), "DEP NOTICE".into());

        // nothing on disk yet: wanted license, notice, and manifest all report stale.
        let stale = stale_outputs(&lic, &texts, &not, &notices, &manifest_path, "MANIFEST");
        assert!(stale.iter().any(|s| s.contains("MIT.txt")));
        assert!(stale.iter().any(|s| s.contains("dep-1.0.0.txt")));
        assert!(stale.iter().any(|s| s.contains("THIRD-PARTY.md")));

        // write exactly what is wanted: nothing stale.
        fs::write(lic.join("MIT.txt"), "MIT TEXT").unwrap();
        fs::write(not.join("dep-1.0.0.txt"), "DEP NOTICE").unwrap();
        fs::write(&manifest_path, "MANIFEST").unwrap();
        assert!(stale_outputs(&lic, &texts, &not, &notices, &manifest_path, "MANIFEST").is_empty());

        // a leftover bundled-license text not in the wanted set is stale (we wrote it).
        fs::write(lic.join("Apache-2.0.txt"), "x").unwrap();
        let stale = stale_outputs(&lic, &texts, &not, &notices, &manifest_path, "MANIFEST");
        assert!(stale.iter().any(|s| s.contains("Apache-2.0.txt")));
        fs::remove_file(lic.join("Apache-2.0.txt")).unwrap();

        // a hand-added file whose stem is not an SPDX id is left alone.
        fs::write(lic.join("NOTICE.txt"), "x").unwrap();
        let stale = stale_outputs(&lic, &texts, &not, &notices, &manifest_path, "MANIFEST");
        assert!(!stale.iter().any(|s| s.contains("NOTICE.txt")));

        // a leftover notice for a dep no longer in the tree is stale (we wrote it),
        // but a hand-added file without a "-<semver>" stem suffix is left alone.
        fs::write(not.join("gone-2.0.0.txt"), "x").unwrap();
        fs::write(not.join("README.txt"), "x").unwrap();
        let stale = stale_outputs(&lic, &texts, &not, &notices, &manifest_path, "MANIFEST");
        assert!(stale.iter().any(|s| s.contains("gone-2.0.0.txt")));
        assert!(!stale.iter().any(|s| s.contains("README.txt")));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn copyright_lines_extracts_holders_and_skips_boilerplate() {
        let mut out = BTreeSet::new();
        copyright_lines(
            "MIT License\n\n  Copyright   (c)  2019 The dep   Developers\nCopyright 2024 Alice\n© 2024 Bob\n\
             Copyright (c) <year> <copyright holders>\nCopyright [yyyy] [name of copyright owner]\n\
             Copyright {yyyy} {name of copyright owner}\nCopyright\nCopyright (c)\n\
             copyright license to reproduce, prepare Derivative Works of,\n\
             copyright notice that is included in or attached to the work\n\
             COPYRIGHT AND PERMISSION NOTICE\nPermission is hereby granted...\n",
            &mut out,
        );
        let got: Vec<&str> = out.iter().map(String::as_str).collect();
        // real holders kept, whitespace normalized; placeholders, bare headings,
        // and wrapped license-body fragments dropped.
        assert_eq!(got, vec!["Copyright (c) 2019 The dep Developers", "Copyright 2024 Alice", "© 2024 Bob"]);

        // duplicates across files dedup via the shared set.
        copyright_lines("Copyright 2024 Alice\n", &mut out);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn display_author_strips_email() {
        assert_eq!(display_author("Alice <alice@example.com>"), "Alice");
        assert_eq!(display_author("Bob"), "Bob");
        assert_eq!(display_author("<only@email.com>"), "<only@email.com>");
    }

    #[test]
    fn stale_notice_requires_a_semver_stem_suffix() {
        let notices: BTreeMap<String, String> = [("foo-bar-1.0.0".to_string(), String::new())].into();
        // ours and unused -> stale; ours and used -> not; no "-<semver>" suffix -> hand-added.
        assert!(is_stale_notice(Path::new("N/dep-2.0.0.txt"), &notices));
        assert!(!is_stale_notice(Path::new("N/foo-bar-1.0.0.txt"), &notices));
        assert!(!is_stale_notice(Path::new("N/NOTICE.txt"), &notices));
        assert!(!is_stale_notice(Path::new("N/readme-notes.txt"), &notices));
        assert!(!is_stale_notice(Path::new("N/dep-2.0.0.md"), &notices));
    }

    #[test]
    fn canonical_text_covers_spdx_licenses_and_exceptions() {
        // the spdx `text` feature, not a hand-bundled set: licenses beyond the old nine
        // and WITH-exception bodies both resolve.
        assert!(canonical_text("MIT").is_some());
        assert!(canonical_text("MPL-2.0").is_some()); // never bundled by hand
        assert!(canonical_text("LLVM-exception").is_some()); // an exception body
        assert!(canonical_text("NotARealLicense").is_none());
    }

    #[test]
    fn exceptions_collected_only_for_the_chosen_license() {
        let expr = |s: &str| spdx::Expression::parse_mode(s, spdx::ParseMode::LAX).unwrap();
        let chosen: BTreeSet<String> = ["Apache-2.0".to_string()].into_iter().collect();
        // the WITH sits on the chosen license -> collected.
        assert_eq!(exceptions_for(&expr("Apache-2.0 WITH LLVM-exception"), &chosen), vec!["LLVM-exception"]);
        // an OR whose exception-bearing side (MIT) is not the chosen one -> not collected.
        let mit_chosen: BTreeSet<String> = ["MIT".to_string()].into_iter().collect();
        assert!(exceptions_for(&expr("(GPL-2.0 WITH GCC-exception-2.0) OR MIT"), &mit_chosen).is_empty());
    }

    #[test]
    fn with_exception_attributes_the_base_license() {
        // `A WITH exception` is one SPDX leaf; it is accepted iff the base license is,
        // and attributes the base's text (the exception grants only extra permission).
        assert_eq!(pick("Apache-2.0 WITH LLVM-exception"), Some(vec!["Apache-2.0".into()]));
        assert_eq!(pick("GPL-3.0-only WITH Classpath-exception-2.0"), None); // base not accepted
    }

    #[test]
    fn relative_inside_rejects_escapes_and_rootlike() {
        assert!(relative_inside("manifest", "THIRD-PARTY.md").is_ok());
        assert!(relative_inside("licenses-dir", "docs/LICENSES").is_ok());
        assert!(relative_inside("manifest", "").is_err()); // no target -> project root
        assert!(relative_inside("licenses-dir", ".").is_err()); // "." -> project root
        assert!(relative_inside("manifest", "../escape.md").is_err());
        assert!(relative_inside("manifest", "/etc/passwd").is_err());
    }

    #[test]
    fn unreadable_config_errors_instead_of_defaulting() {
        use cargo_metadata::camino::Utf8PathBuf;
        let dir = std::env::temp_dir().join(format!("tribute-cfg-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.clone()).unwrap();

        // a present-but-non-UTF-8 tribute.toml must not be silently ignored.
        fs::write(dir.join("tribute.toml"), [0xff, 0xfe, 0x41, 0x00]).unwrap();
        assert!(load_settings(&root).is_err());

        // a missing config still falls back to defaults.
        fs::remove_file(dir.join("tribute.toml")).unwrap();
        assert!(load_settings(&root).is_ok());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn matches_output_ignores_crlf() {
        // a CRLF checkout (git autocrlf) of an LF-written file is not stale.
        assert!(matches_output(Some("a\r\nb\r\n".into()), "a\nb\n"));
        assert!(matches_output(Some("a\nb\n".into()), "a\nb\n"));
        assert!(!matches_output(Some("a\nb\n".into()), "a\nDIFFERENT\n"));
        assert!(!matches_output(None, "x")); // missing file is stale
    }
}
