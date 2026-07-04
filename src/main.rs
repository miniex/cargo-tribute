//! From a Cargo dependency tree, write a REUSE-style LICENSES/ folder (one canonical
//! text per license used) and a per-crate attribution manifest. `--check` verifies the
//! output is current and every license is accepted, without writing anything.

use cargo_metadata::camino::Utf8Path;
use cargo_metadata::{DependencyKind, MetadataCommand, Package, PackageId};
use serde::Deserialize;
use spdx::expression::{ExprNode, Operator};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

// default allowed licenses; also the OR preference order (earlier wins when an
// "A OR B" can pick either). Overridable via tribute.toml.
const DEFAULT_ACCEPTED: &[&str] =
    &["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "0BSD", "Zlib", "Unlicense", "Unicode-3.0"];

fn canonical_text(id: &str) -> Option<&'static str> {
    Some(match id {
        "MIT" => include_str!("../assets/licenses/MIT.txt"),
        "Apache-2.0" => include_str!("../assets/licenses/Apache-2.0.txt"),
        "BSD-2-Clause" => include_str!("../assets/licenses/BSD-2-Clause.txt"),
        "BSD-3-Clause" => include_str!("../assets/licenses/BSD-3-Clause.txt"),
        "ISC" => include_str!("../assets/licenses/ISC.txt"),
        "0BSD" => include_str!("../assets/licenses/0BSD.txt"),
        "Zlib" => include_str!("../assets/licenses/Zlib.txt"),
        "Unlicense" => include_str!("../assets/licenses/Unlicense.txt"),
        "Unicode-3.0" => include_str!("../assets/licenses/Unicode-3.0.txt"),
        _ => return None,
    })
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Config {
    accepted: Option<Vec<String>>,
    manifest: Option<String>,
    #[serde(rename = "licenses-dir")]
    licenses_dir: Option<String>,
}

struct Settings {
    accepted: Vec<String>,
    manifest: PathBuf,     // absolute output path
    manifest_link: String, // relative name, for messages
    licenses_dir: PathBuf, // absolute output dir
    licenses_link: String, // relative name, for markdown links + messages
}

// anchor tribute.toml and outputs to the workspace root, not the cwd, so
// --manifest-path against a crate elsewhere reads and writes beside that crate.
fn load_settings(root: &Utf8Path) -> Result<Settings, String> {
    let cfg: Config = match fs::read_to_string(root.join("tribute.toml")) {
        Ok(s) => toml::from_str(&s).map_err(|e| format!("tribute.toml: {e}"))?,
        Err(_) => Config::default(),
    };
    let manifest_link = cfg.manifest.unwrap_or_else(|| "THIRD-PARTY.md".into());
    let licenses_link = cfg.licenses_dir.unwrap_or_else(|| "LICENSES".into());
    Ok(Settings {
        accepted: cfg.accepted.unwrap_or_else(|| DEFAULT_ACCEPTED.iter().map(|s| s.to_string()).collect()),
        manifest: root.join(&manifest_link).into(),
        licenses_dir: root.join(&licenses_link).into(),
        manifest_link,
        licenses_link,
    })
}

const HELP: &str = "\
cargo-tribute -- REUSE-style third-party license attribution from a Cargo tree

USAGE:
    cargo tribute [OPTIONS]

OPTIONS:
        --check              verify the output is current; do not write (exit 1 if stale)
        --manifest-path <P>  path to Cargo.toml (default: auto-detect from the cwd)
    -h, --help               print this help
    -V, --version            print version

CONFIG (tribute.toml in the project root, all optional):
    accepted = [\"MIT\", \"Apache-2.0\", ...]   # allowed licenses; also the OR preference order
    manifest = \"THIRD-PARTY.md\"              # attribution manifest path
    licenses-dir = \"LICENSES\"                # folder for the canonical license texts
";

fn main() -> ExitCode {
    let mut check = false;
    let mut manifest_path = None;
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().map(String::as_str) == Some("tribute") {
        args.next(); // cargo passes the subcommand name when invoked as `cargo tribute`
    }
    while let Some(a) = args.next() {
        match a.as_str() {
            "--check" => check = true,
            "--manifest-path" => manifest_path = args.next(),
            "-h" | "--help" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("cargo-tribute {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("cargo-tribute: unknown argument '{other}' (try --help)");
                return ExitCode::FAILURE;
            }
        }
    }
    match run(check, manifest_path) {
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

fn run(check: bool, manifest_path: Option<String>) -> Result<String, String> {
    let mut cmd = MetadataCommand::new();
    if let Some(p) = manifest_path {
        cmd.manifest_path(PathBuf::from(p));
    }
    let meta = cmd.exec().map_err(|e| e.to_string())?;
    let set = load_settings(&meta.workspace_root)?;
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
    let mut by_license: BTreeMap<String, Vec<&Package>> = BTreeMap::new();
    let mut failures = Vec::new();
    for id in &deps {
        let pkg = pkg_of[id];
        let name = format!("{} {}", pkg.name, pkg.version);
        let Some(expr_str) = pkg.license.as_deref() else {
            failures.push(format!("{name}: no license field"));
            continue;
        };
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
                for lic in chosen {
                    by_license.entry(lic).or_default().push(pkg);
                }
            }
            None => failures.push(format!("{name}: license '{expr_str}' not in the accepted set")),
        }
    }
    if !failures.is_empty() {
        return Err(format!("license policy failed:\n  {}", failures.join("\n  ")));
    }

    // resolve each used license to its canonical text.
    let mut texts: BTreeMap<&str, &'static str> = BTreeMap::new();
    for id in by_license.keys() {
        let text = canonical_text(id)
            .ok_or_else(|| format!("no canonical text bundled for '{id}' (add assets/licenses/{id}.txt)"))?;
        texts.insert(id, text);
    }
    let manifest = render_manifest(&by_license, &set.licenses_link);

    if check {
        let stale = stale_outputs(&set.licenses_dir, &texts, &set.manifest, &manifest);
        if !stale.is_empty() {
            return Err(format!("out of date (run `cargo tribute`):\n  {}", stale.join("\n  ")));
        }
        Ok(format!("up to date: {} licenses, {} crates", texts.len(), deps.len()))
    } else {
        fs::create_dir_all(&set.licenses_dir).map_err(|e| e.to_string())?;
        // drop stale license files no longer used
        if let Ok(entries) = fs::read_dir(&set.licenses_dir) {
            for e in entries.flatten() {
                let p = e.path();
                let keep = p.file_stem().and_then(|s| s.to_str()).is_some_and(|s| texts.contains_key(s));
                if p.extension().is_some_and(|x| x == "txt") && !keep {
                    let _ = fs::remove_file(p);
                }
            }
        }
        for (id, text) in &texts {
            fs::write(set.licenses_dir.join(format!("{id}.txt")), text).map_err(|e| e.to_string())?;
        }
        fs::write(&set.manifest, &manifest).map_err(|e| e.to_string())?;
        Ok(format!(
            "wrote {}/ ({} licenses) and {} ({} crates)",
            set.licenses_link,
            texts.len(),
            set.manifest_link,
            deps.len()
        ))
    }
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

// paths a plain run would create, change, or delete; empty means --check passes.
// includes orphaned <id>.txt files the write path removes, so --check cannot pass
// while stale license files still sit in the tree.
fn stale_outputs(
    licenses_dir: &Path,
    texts: &BTreeMap<&str, &'static str>,
    manifest_path: &Path,
    manifest: &str,
) -> Vec<String> {
    let mut stale = Vec::new();
    for (id, want) in texts {
        let path = licenses_dir.join(format!("{id}.txt"));
        if fs::read_to_string(&path).ok().as_deref() != Some(*want) {
            stale.push(path.display().to_string());
        }
    }
    if let Ok(entries) = fs::read_dir(licenses_dir) {
        for e in entries.flatten() {
            let p = e.path();
            let orphan = p.extension().is_some_and(|x| x == "txt")
                && !p.file_stem().and_then(|s| s.to_str()).is_some_and(|s| texts.contains_key(s));
            if orphan {
                stale.push(p.display().to_string());
            }
        }
    }
    if fs::read_to_string(manifest_path).ok().as_deref() != Some(manifest) {
        stale.push(manifest_path.display().to_string());
    }
    stale
}

fn render_manifest(by_license: &BTreeMap<String, Vec<&Package>>, licenses_dir: &str) -> String {
    let mut out = String::from(
        "# Third-party licenses\n\nDependencies linked into this crate, grouped by license; full texts are in \
         [`",
    );
    out.push_str(licenses_dir);
    out.push_str("/`](");
    out.push_str(licenses_dir);
    out.push_str("). Generated by `cargo tribute`; do not edit.\n\n");
    for (id, pkgs) in by_license {
        let mut ps: Vec<&Package> = pkgs.clone();
        ps.sort_by(|a, b| (&*a.name, &a.version).cmp(&(&*b.name, &b.version)));
        ps.dedup_by(|a, b| a.id == b.id);
        out.push_str(&format!("## {id}\n\nText: [`{licenses_dir}/{id}.txt`]({licenses_dir}/{id}.txt)\n\n"));
        for p in ps {
            let url = p.repository.clone().unwrap_or_else(|| format!("https://crates.io/crates/{}", p.name));
            // show the declared SPDX when it differs from the section license, so WITH
            // exceptions and dual-license picks are not hidden by the grouping. only the
            // base license text is emitted; an exception text would need adding to assets/.
            match p.license.as_deref().filter(|e| *e != id.as_str()) {
                Some(expr) => out.push_str(&format!("- [{} {}]({url}) — `{expr}`\n", p.name, p.version)),
                None => out.push_str(&format!("- [{} {}]({url})\n", p.name, p.version)),
            }
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
    fn stale_detects_missing_and_orphan() {
        let dir = std::env::temp_dir().join(format!("tribute-test-{}", std::process::id()));
        let lic = dir.join("LICENSES");
        fs::create_dir_all(&lic).unwrap();
        let manifest_path = dir.join("THIRD-PARTY.md");
        let mut texts: BTreeMap<&str, &'static str> = BTreeMap::new();
        texts.insert("MIT", "MIT TEXT");

        // nothing on disk yet: wanted license and manifest both report stale.
        let stale = stale_outputs(&lic, &texts, &manifest_path, "MANIFEST");
        assert!(stale.iter().any(|s| s.contains("MIT.txt")));
        assert!(stale.iter().any(|s| s.contains("THIRD-PARTY.md")));

        // write exactly what is wanted: nothing stale.
        fs::write(lic.join("MIT.txt"), "MIT TEXT").unwrap();
        fs::write(&manifest_path, "MANIFEST").unwrap();
        assert!(stale_outputs(&lic, &texts, &manifest_path, "MANIFEST").is_empty());

        // a leftover license file not in the wanted set is stale too.
        fs::write(lic.join("GPL-3.0.txt"), "x").unwrap();
        let stale = stale_outputs(&lic, &texts, &manifest_path, "MANIFEST");
        assert!(stale.iter().any(|s| s.contains("GPL-3.0.txt")));

        fs::remove_dir_all(&dir).ok();
    }
}
