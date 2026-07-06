//! From a Cargo dependency tree, write a REUSE-style LICENSES/ folder (one canonical
//! text per license used), a NOTICES/ folder (NOTICE files deps ship), and a per-crate
//! attribution manifest with copyright lines. `--check` verifies the output is current
//! and every license is accepted, without writing anything.

mod config;
mod harvest;
mod output;
mod policy;

use cargo_metadata::{DependencyKind, MetadataCommand, Package, PackageId};
use config::{Accept, Extra, clarify_expr, load_settings, parse_accept, policy_matches, warn_unknown_ids};
use harvest::{Extras, harvest_extras};
use output::{
    Resolution, io, is_stale_license, is_stale_notice, render_cyclonedx, render_json, render_manifest, render_text,
    stale_outputs,
};
use policy::{canonical_text, choose, exceptions_for, license_name};
use spdx::expression::ExprNode;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

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
        --json               shorthand for --format json
        --format <F>         print the resolved attribution as F instead of writing
                             files: json, text (flat list + full texts, for an
                             \"open source licenses\" screen), or cyclonedx
                             (CycloneDX 1.6 SBOM carrying the license texts)
    -h, --help               print this help
    -V, --version            print version

CONFIG (tribute.toml in the project root, all optional):
    accepted = [\"MIT\", \"Apache-2.0\", ...]    # allowed licenses, or \"A WITH B\" pairings;
                                             # also the OR preference order
    include-dev = false                      # also attribute dev-dependencies
    include-build = false                    # also attribute build-dependencies
    manifest = \"THIRD-PARTY.md\"              # attribution manifest path
    licenses-dir = \"LICENSES\"                # folder for the canonical license texts
    notices-dir = \"NOTICES\"                  # folder for NOTICE files shipped by dependencies

    [[clarify]]                              # override a crate's license (missing/wrong/non-SPDX)
    name = \"ring\"
    version = \"0.17.8\"                       # optional semver req; omit to match any version
    expression = \"MIT AND ISC AND OpenSSL\"

    [[exception]]                            # allow extra licenses for one crate only
    name = \"unicode-ident\"
    allow = [\"Unicode-DFS-2016\"]

    [[extra]]                                # attribute non-crate code (vendored C, ...)
    name = \"zlib (bundled in libz-sys)\"
    expression = \"Zlib\"
    url = \"https://zlib.net\"                 # optional, like copyright = \"...\"

    [[license-text]]                         # local text for a LicenseRef-* id
    id = \"LicenseRef-weird\"
    file = \"licenses-extra/weird.txt\"
";

// a stdout report mode: print the resolved attribution instead of writing files.
enum Format {
    Json,
    Text,
    CycloneDx,
}

fn main() -> ExitCode {
    let mut check = false;
    let mut format = None;
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
            "--json" => format = Some(Format::Json),
            "--format" => match args.next().as_deref() {
                Some("json") => format = Some(Format::Json),
                Some("text") => format = Some(Format::Text),
                Some("cyclonedx") => format = Some(Format::CycloneDx),
                Some(v) => {
                    eprintln!("cargo-tribute: unknown format '{v}' (expected json, text, or cyclonedx)");
                    return ExitCode::FAILURE;
                }
                None => {
                    eprintln!("cargo-tribute: --format needs a value");
                    return ExitCode::FAILURE;
                }
            },
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
    match run(check, format, manifest_path, cargo_flags) {
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

fn run(
    check: bool,
    format: Option<Format>,
    manifest_path: Option<String>,
    cargo_flags: Vec<String>,
) -> Result<String, String> {
    let mut cmd = MetadataCommand::new();
    if let Some(p) = manifest_path {
        cmd.manifest_path(PathBuf::from(p));
    }
    if !cargo_flags.is_empty() {
        cmd.other_options(cargo_flags);
    }
    let meta = cmd.exec().map_err(|e| e.to_string())?;
    let set = load_settings(&meta.workspace_root)?;
    // a typo in a policy entry (e.g. "Apache2.0") would silently reject that license; flag it.
    for a in &set.accepted {
        warn_unknown_ids("accepted", a);
    }
    for x in &set.exception {
        for a in x.allow.iter().map(|s| parse_accept(s)) {
            warn_unknown_ids("exception-allowed", &a);
        }
    }
    let resolve = meta.resolve.as_ref().ok_or("no dependency resolution (need a Cargo.toml)")?;

    let node_of: BTreeMap<&PackageId, _> = resolve.nodes.iter().map(|n| (&n.id, n)).collect();
    let pkg_of: BTreeMap<&PackageId, &Package> = meta.packages.iter().map(|p| (&p.id, p)).collect();
    let workspace: BTreeSet<&PackageId> = meta.workspace_members.iter().collect();

    // dependency closure of the workspace members, minus the members themselves.
    // normal deps always; dev and build deps only when opted in via tribute.toml.
    let mut seen = BTreeSet::new();
    let mut stack: Vec<&PackageId> = meta.workspace_members.iter().collect();
    let mut deps = BTreeSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(node) = node_of.get(id) else { continue };
        for dep in &node.deps {
            let wanted = dep.dep_kinds.iter().any(|k| match k.kind {
                DependencyKind::Normal => true,
                DependencyKind::Development => set.include_dev,
                DependencyKind::Build => set.include_build,
                _ => false,
            });
            if !wanted {
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
    // every (license, WITH-exception) leaf seen across the tree, for the
    // unused-accepted warning.
    let mut encountered: BTreeSet<(String, Option<String>)> = BTreeSet::new();
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
        for node in expr.iter() {
            if let ExprNode::Req(r) = node {
                let ex = r.req.addition.as_ref().and_then(|a| a.id()).map(|e| e.name.to_string());
                encountered.insert((license_name(&r.req), ex));
            }
        }
        // [[exception]] entries widen the accepted set for this crate only, appended
        // after the global list so a globally-accepted license still wins the OR pick.
        let extra: Vec<Accept> = set
            .exception
            .iter()
            .filter(|x| policy_matches(&x.name, x.version.as_deref(), pkg.name.as_ref(), &pkg.version))
            .flat_map(|x| x.allow.iter().map(|s| parse_accept(s)))
            .collect();
        let chosen = if extra.is_empty() {
            choose(&expr, &set.accepted)
        } else {
            let mut acc = set.accepted.clone();
            acc.extend(extra);
            choose(&expr, &acc)
        };
        match chosen {
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
    // [[extra]] entries attribute code the crate graph can't see (vendored C, bundled
    // assets); the expression takes the same parse/choose path as a crate's.
    let mut extra_by_license: BTreeMap<String, Vec<&Extra>> = BTreeMap::new();
    let mut extra_chosen: Vec<(&Extra, BTreeSet<String>)> = Vec::new();
    for x in &set.extra {
        let expr = match spdx::Expression::parse_mode(&x.expression, spdx::ParseMode::LAX) {
            Ok(e) => e,
            Err(e) => {
                failures.push(format!("[[extra]] {}: unparsable SPDX '{}' ({e})", x.name, x.expression));
                continue;
            }
        };
        for node in expr.iter() {
            if let ExprNode::Req(r) = node {
                let ex = r.req.addition.as_ref().and_then(|a| a.id()).map(|e| e.name.to_string());
                encountered.insert((license_name(&r.req), ex));
            }
        }
        match choose(&expr, &set.accepted) {
            Some(chosen) => {
                for ex in exceptions_for(&expr, &chosen) {
                    used_exceptions.insert(ex);
                }
                for lic in &chosen {
                    extra_by_license.entry(lic.clone()).or_default().push(x);
                }
                extra_chosen.push((x, chosen));
            }
            None => failures.push(format!("[[extra]] {}: license '{}' not in the accepted set", x.name, x.expression)),
        }
    }
    // warn on clarify/exception entries that matched no dependency, so a typo in name
    // or version is visible instead of silently ignored.
    let no_match = |name: &str, version: Option<&str>| {
        !deps
            .iter()
            .any(|id| pkg_of.get(id).is_some_and(|p| policy_matches(name, version, p.name.as_ref(), &p.version)))
    };
    for c in &set.clarify {
        if no_match(&c.name, c.version.as_deref()) {
            let ver = c.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default();
            eprintln!("cargo-tribute: warning: clarify for '{}{ver}' matched no dependency", c.name);
        }
    }
    for x in &set.exception {
        if no_match(&x.name, x.version.as_deref()) {
            let ver = x.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default();
            eprintln!("cargo-tribute: warning: exception for '{}{ver}' matched no dependency", x.name);
        }
    }
    // warn on accepted entries no dependency even references, so a stale allowlist is
    // visible. explicit lists only; the built-in default may sit partly unused.
    if set.accepted_explicit {
        for a in &set.accepted {
            if !encountered.iter().any(|(l, e)| a.allows(l, e.as_deref())) {
                eprintln!("cargo-tribute: warning: accepted license '{}' matched no dependency", a.raw);
            }
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

    // texts for ids outside the SPDX corpus (LicenseRef-*), from [[license-text]].
    // LF-normalize like a NOTICE, so a CRLF file can't read as stale in --check.
    let mut custom: BTreeMap<&str, String> = BTreeMap::new();
    for t in &set.license_text {
        let p = meta.workspace_root.join(&t.file);
        custom.insert(&t.id, io(p.as_std_path(), fs::read_to_string(&p))?.replace("\r\n", "\n"));
    }
    // resolve each used license and exception id to its text.
    let mut texts: BTreeMap<&str, String> = BTreeMap::new();
    for id in by_license
        .keys()
        .chain(extra_by_license.keys())
        .map(String::as_str)
        .chain(used_exceptions.iter().map(String::as_str))
    {
        let text = canonical_text(id)
            .map(str::to_string)
            .or_else(|| custom.get(id).cloned())
            .ok_or_else(|| format!("no text for '{id}' (add a [[license-text]] entry to tribute.toml)"))?;
        texts.insert(id, text);
    }
    // warn on license-text entries nothing uses, like the other policy no-match warnings.
    for t in &set.license_text {
        if !texts.contains_key(t.id.as_str()) {
            eprintln!("cargo-tribute: warning: license-text for '{}' matched no dependency", t.id);
        }
    }
    let res = Resolution {
        deps: &deps,
        pkg_of: &pkg_of,
        effective: &effective,
        chosen_of: &chosen_of,
        by_license: &by_license,
        extra_by_license: &extra_by_license,
        extra_chosen: &extra_chosen,
        used_exceptions: &used_exceptions,
        extras: &extras,
    };
    // --format/--json is a read-only report of what the tree resolves to; it never
    // writes or checks.
    if let Some(f) = format {
        return match f {
            Format::Json => render_json(&res),
            Format::Text => Ok(render_text(&res, &texts)),
            Format::CycloneDx => render_cyclonedx(&res, &texts),
        };
    }
    let manifest = render_manifest(&res, &set.licenses_link, &set.notices_link);

    if check {
        let stale = stale_outputs(&set.licenses_dir, &texts, &set.notices_dir, &notices, &set.manifest, &manifest);
        if !stale.is_empty() {
            return Err(format!("out of date (run `cargo tribute`):\n  {}", stale.join("\n  ")));
        }
        let n = if notices.is_empty() { String::new() } else { format!(", {} notices", notices.len()) };
        let e = if extra_chosen.is_empty() { String::new() } else { format!(", {} extras", extra_chosen.len()) };
        Ok(format!("up to date: {} license texts{n}, {} crates{e}", texts.len(), deps.len()))
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
        let e = if extra_chosen.is_empty() { String::new() } else { format!(", {} extras", extra_chosen.len()) };
        Ok(format!(
            "wrote {}/ ({} license texts){n} and {} ({} crates{e})",
            set.licenses_link,
            texts.len(),
            set.manifest_link,
            deps.len()
        ))
    }
}
