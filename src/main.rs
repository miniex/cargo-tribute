//! From a Cargo dependency tree, write a REUSE-style LICENSES/ folder (one canonical
//! text per license used), a NOTICES/ folder (NOTICE files deps ship), and a per-crate
//! attribution manifest with copyright lines. `--check` verifies the output is current
//! and every license is accepted, without writing anything.

#[cfg(feature = "audit")]
mod audit;
mod config;
mod harvest;
mod output;
mod policy;

use cargo_metadata::{DependencyKind, MetadataCommand, Package, PackageId, TargetKind};
use config::{
    Accept, Extra, Layout, apply_deny, clarify_expr, load_settings, parse_accept, policy_matches, warn_unknown_ids,
};
use harvest::{Extras, harvest_extras};
use output::{
    Resolution, io, is_stale_license, is_stale_notice, render_cyclonedx, render_json, render_manifest, render_text,
    stale_doc, stale_outputs,
};
use policy::{canonical_text, choose, exceptions_for, license_name};
use spdx::expression::ExprNode;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const AFTER_HELP: &str = "\
EXIT CODES:
    1 license policy failed, 2 output out of date (--check), 3 anything else

CONFIG (tribute.toml in the project root, all optional):
    accepted = [\"MIT\", \"Apache-2.0\", ...]    # allowed licenses, or \"A WITH B\" pairings;
                                             # also the OR preference order
    include-dev = false                      # also attribute dev-dependencies
    include-build = false                    # also attribute build-dependencies
    skip-private = false                     # skip path/git/non-crates.io dependencies
    skip-proc-macros = false                 # skip proc-macro crates (compile-time only)
    manifest = \"THIRD-PARTY.md\"              # attribution manifest path
    licenses-dir = \"LICENSES\"                # folder for the canonical license texts
    notices-dir = \"NOTICES\"                  # folder for NOTICE files shipped by dependencies
    layout = \"folders\"                       # what a run writes and --check gates:
                                             # folders (default) = the three outputs above;
                                             # flat = one all-in-one THIRD-PARTY-NOTICES
                                             # file (the --format text document); both
    flat-file = \"THIRD-PARTY-NOTICES\"        # the flat file's path (layout flat/both)

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
    url = \"https://zlib.net\"                 # optional; copyright and notes (free text
                                             # for the notices file) are optional too

    [[license-text]]                         # local text for a LicenseRef-* id
    id = \"LicenseRef-weird\"
    file = \"licenses-extra/weird.txt\"
";

// a stdout report mode: print the resolved attribution instead of writing files.
#[derive(Clone, Copy, clap::ValueEnum)]
enum Format {
    /// the resolved attribution as JSON
    Json,
    /// one flat THIRD-PARTY-NOTICES document (per-package entries + license texts)
    Text,
    /// a CycloneDX 1.6 SBOM carrying the license texts
    #[value(name = "cyclonedx")]
    CycloneDx,
}

// what run() produced: a report always goes to stdout, a summary only without -q.
enum Output {
    Report(String),
    Summary(String),
}

// failure kinds map to distinct exit codes, so CI can branch on them.
enum Failure {
    Policy(String), // 1: a dependency's license is not accepted
    Stale(String),  // 2: --check found the committed output out of date
    Other(String),  // 3: io/config/metadata errors
}

impl From<String> for Failure {
    fn from(s: String) -> Self {
        Failure::Other(s)
    }
}

impl From<&str> for Failure {
    fn from(s: &str) -> Self {
        Failure::Other(s.into())
    }
}

#[derive(clap::Parser)]
#[command(
    name = "cargo-tribute",
    bin_name = "cargo tribute",
    version,
    about = "REUSE-style third-party license attribution from a Cargo tree",
    after_help = AFTER_HELP
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,

    /// verify the output is current; do not write (exit 2 if stale)
    #[arg(long)]
    check: bool,

    /// compare declared licenses against the license files the crates actually ship
    /// (advisory report; never fails)
    #[arg(long)]
    audit: bool,

    /// attribute only this workspace member's dependencies (repeatable; report-only,
    /// use with --json/--format/--audit)
    #[arg(short, long = "package", value_name = "NAME")]
    packages: Vec<String>,

    /// take the accepted list and per-crate exceptions from a cargo-deny deny.toml
    /// [licenses] section
    #[arg(long, value_name = "PATH")]
    from_deny: Option<String>,

    /// path to Cargo.toml (default: auto-detect from the cwd)
    #[arg(long, value_name = "PATH", global = true)]
    manifest_path: Option<String>,

    /// forwarded to `cargo metadata`, so CI resolves deterministically and offline
    #[arg(long, global = true)]
    locked: bool,
    /// forwarded to `cargo metadata`
    #[arg(long, global = true)]
    offline: bool,
    /// forwarded to `cargo metadata`
    #[arg(long, global = true)]
    frozen: bool,

    /// forwarded to `cargo metadata`, to attribute feature-gated deps (repeatable)
    #[arg(long, value_name = "FEATURES")]
    features: Vec<String>,
    /// forwarded to `cargo metadata`
    #[arg(long)]
    all_features: bool,
    /// forwarded to `cargo metadata`
    #[arg(long)]
    no_default_features: bool,
    /// forwarded to `cargo metadata`, for platform-specific deps (repeatable)
    #[arg(long, value_name = "TRIPLE")]
    filter_platform: Vec<String>,

    /// print the resolved attribution as FORMAT instead of writing files
    #[arg(long, value_name = "FORMAT")]
    format: Option<Format>,
    /// shorthand for --format json
    #[arg(long)]
    json: bool,

    /// suppress the success summary
    #[arg(short, long, global = true)]
    quiet: bool,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// scaffold a commented tribute.toml at the workspace root
    Init,
}

// the flags forwarded verbatim to `cargo metadata`, reassembled from the parse.
fn cargo_flags(cli: &Cli) -> Vec<String> {
    let mut flags: Vec<String> = Vec::new();
    let bools = [
        ("--locked", cli.locked),
        ("--offline", cli.offline),
        ("--frozen", cli.frozen),
        ("--all-features", cli.all_features),
        ("--no-default-features", cli.no_default_features),
    ];
    for (flag, on) in bools {
        if on {
            flags.push(flag.into());
        }
    }
    for f in &cli.features {
        flags.extend(["--features".into(), f.clone()]);
    }
    for t in &cli.filter_platform {
        flags.extend(["--filter-platform".into(), t.clone()]);
    }
    flags
}

fn main() -> ExitCode {
    use clap::Parser;
    let mut argv: Vec<String> = std::env::args().collect();
    if argv.get(1).map(String::as_str) == Some("tribute") {
        argv.remove(1); // cargo passes the subcommand name when invoked as `cargo tribute`
    }
    let cli = match Cli::try_parse_from(argv) {
        Ok(cli) => cli,
        Err(e) => {
            // clap's own exit code for a parse error is 2, which reads as "stale" in
            // our contract; keep cli mistakes at 3. --help/--version print and exit 0.
            let _ = e.print();
            return ExitCode::from(if e.use_stderr() { 3 } else { 0 });
        }
    };
    let quiet = cli.quiet;
    match run(cli) {
        Ok(Output::Report(s)) => {
            println!("{s}");
            ExitCode::SUCCESS
        }
        Ok(Output::Summary(s)) => {
            if !quiet {
                println!("{s}");
            }
            ExitCode::SUCCESS
        }
        Err(f) => {
            let (msg, code) = match f {
                Failure::Policy(m) => (m, 1),
                Failure::Stale(m) => (m, 2),
                Failure::Other(m) => (m, 3),
            };
            eprintln!("cargo-tribute: {msg}");
            ExitCode::from(code)
        }
    }
}

// scaffold a commented tribute.toml at the workspace root; refuses to overwrite.
fn run_init(cli: &Cli) -> Result<Output, Failure> {
    let mut cmd = MetadataCommand::new();
    if let Some(p) = &cli.manifest_path {
        cmd.manifest_path(PathBuf::from(p));
    }
    // forward --locked/--offline/--frozen, so init works where the tree resolves.
    let flags = cargo_flags(cli);
    if !flags.is_empty() {
        cmd.other_options(flags);
    }
    let meta = cmd.exec().map_err(|e| e.to_string())?;
    let path = meta.workspace_root.join("tribute.toml");
    if path.as_std_path().exists() {
        return Err(Failure::Other(format!("{path} already exists")));
    }
    io(path.as_std_path(), fs::write(&path, INIT_TEMPLATE))?;
    Ok(Output::Summary(format!("wrote {path}; uncomment what you need (CI: cargo tribute --locked --check)")))
}

const INIT_TEMPLATE: &str = "\
# cargo-tribute configuration; every field is optional, these are the defaults.

# allowed licenses, also the OR preference order; \"A WITH B\" pairings work too.
# accepted = [\"MIT\", \"Apache-2.0\", \"BSD-2-Clause\", \"BSD-3-Clause\", \"ISC\", \"0BSD\", \"Zlib\", \"Unlicense\", \"Unicode-3.0\"]

# include-dev = false           # also attribute dev-dependencies
# include-build = false         # also attribute build-dependencies
# skip-private = false          # skip path/git/non-crates.io dependencies
# skip-proc-macros = false      # skip proc-macro crates (compile-time only)
# manifest = \"THIRD-PARTY.md\"
# licenses-dir = \"LICENSES\"
# notices-dir = \"NOTICES\"

# what a run writes and --check gates: folders (default) for the three outputs
# above, flat for one all-in-one THIRD-PARTY-NOTICES file, or both:
# layout = \"folders\"
# flat-file = \"THIRD-PARTY-NOTICES\"

# override a crate's license (missing/wrong/non-SPDX):
# [[clarify]]
# name = \"ring\"
# version = \"0.17.8\"
# expression = \"MIT AND ISC AND OpenSSL\"

# allow extra licenses for one crate only:
# [[exception]]
# name = \"unicode-ident\"
# allow = [\"Unicode-DFS-2016\"]

# attribute non-crate code (vendored C, bundled assets); `notes` is free text
# reproduced in the notices file:
# [[extra]]
# name = \"zlib (bundled in libz-sys)\"
# expression = \"Zlib\"
# url = \"https://zlib.net\"
# notes = \"\"\"
# Vendored under third_party/zlib; local patches: none.
# \"\"\"

# local text for a LicenseRef-* id:
# [[license-text]]
# id = \"LicenseRef-weird\"
# file = \"licenses-extra/weird.txt\"
";

fn run(cli: Cli) -> Result<Output, Failure> {
    if matches!(cli.command, Some(Cmd::Init)) {
        return run_init(&cli);
    }
    // --json folds into --format here; everything below sees one field.
    let format = cli.format.or(cli.json.then_some(Format::Json));
    // -p is a scoped, partial view: allow it only with a stdout report. writing or
    // --check would clobber (or perpetually fail against) the shared, whole-workspace
    // LICENSES/NOTICES/manifest.
    if !cli.packages.is_empty() && format.is_none() && !cli.audit {
        return Err("-p is a scoped view; use it with --json/--format/--audit, or drop -p for the full run".into());
    }
    let mut cmd = MetadataCommand::new();
    if let Some(p) = &cli.manifest_path {
        cmd.manifest_path(PathBuf::from(p));
    }
    let flags = cargo_flags(&cli);
    if !flags.is_empty() {
        cmd.other_options(flags);
    }
    let meta = cmd.exec().map_err(|e| e.to_string())?;
    let mut set = load_settings(&meta.workspace_root)?;
    if let Some(p) = &cli.from_deny {
        // like tribute.toml, a relative deny.toml is anchored to the workspace root.
        let p = Path::new(p);
        let p = if p.is_absolute() { p.to_path_buf() } else { meta.workspace_root.as_std_path().join(p) };
        apply_deny(&mut set, &p)?;
    }
    let set = set;
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

    // closure roots: all workspace members, or just the ones named with -p.
    let mut stack: Vec<&PackageId> = if cli.packages.is_empty() {
        meta.workspace_members.iter().collect()
    } else {
        let mut roots = Vec::new();
        for want in &cli.packages {
            let matched: Vec<&PackageId> = meta
                .workspace_members
                .iter()
                .filter(|id| pkg_of.get(id).is_some_and(|p| p.name.as_ref() == want.as_str()))
                .collect();
            if matched.is_empty() {
                return Err(Failure::Other(format!("-p '{want}' matches no workspace member")));
            }
            roots.extend(matched);
        }
        roots
    };

    // dependency closure of the roots, minus the workspace members themselves.
    // normal deps always; dev and build deps only when opted in via tribute.toml.
    let mut seen = BTreeSet::new();
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
            let pkg = pkg_of.get(&dep.pkg);
            // a skipped proc-macro takes its compile-time subtree with it; anything
            // also reachable at runtime stays via its other path.
            if set.skip_proc_macros
                && pkg.is_some_and(|p| p.targets.iter().any(|t| t.kind.contains(&TargetKind::ProcMacro)))
            {
                continue;
            }
            // a private (path/git/alt-registry) dep is first-party: not attributed,
            // but still walked, since its crates.io deps do ship.
            let private = set.skip_private && pkg.is_some_and(|p| !p.source.as_ref().is_some_and(|s| s.is_crates_io()));
            if !workspace.contains(&dep.pkg) && !private {
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
    // or version is visible instead of silently ignored. matched against every package
    // in the graph, not just the attributed deps, so an entry for a crate excluded by
    // -p or a skip option does not read as a typo.
    let no_match = |name: &str, version: Option<&str>| {
        !meta.packages.iter().any(|p| policy_matches(name, version, p.name.as_ref(), &p.version))
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
    // visible. explicit lists only, and not under -p: a scoped run sees a partial
    // tree, so "unused" would be noise.
    let scoped = !cli.packages.is_empty();
    if set.accepted_explicit && !scoped {
        for a in &set.accepted {
            if !encountered.iter().any(|(l, e)| a.allows(l, e.as_deref())) {
                eprintln!("cargo-tribute: warning: accepted license '{}' matched no dependency", a.raw);
            }
        }
    }

    // --audit reports declared-vs-shipped mismatches and deliberately ignores the
    // policy gate: it is about what the crates carry, not what we accept. it needs the
    // opt-in `audit` feature (text detection); prebuilt release binaries ship with it.
    if cli.audit {
        #[cfg(feature = "audit")]
        return Ok(Output::Report(audit::run_audit(&deps, &pkg_of, &effective)));
        #[cfg(not(feature = "audit"))]
        return Err(Failure::Other(
            "this build has no --audit; install with `cargo install cargo-tribute --features audit`".into(),
        ));
    }

    if !failures.is_empty() {
        match format {
            // json/cyclonedx describe the tree as it is (an SBOM of an unaccepted
            // tree is still an SBOM); the failures downgrade to warnings. text and
            // the write path are attribution deliverables and stay gated.
            Some(Format::Json) | Some(Format::CycloneDx) => {
                for f in &failures {
                    eprintln!("cargo-tribute: warning: {f}");
                }
            }
            _ => {
                return Err(Failure::Policy(format!(
                    "license policy failed:\n  {}\n  (fix in tribute.toml: `accepted` allows a license everywhere, \
                     [[exception]] for one crate, [[clarify]] when the declared expression is wrong)",
                    failures.join("\n  ")
                )));
            }
        }
    }

    // per-crate copyright lines and NOTICE bodies, from the local sources.
    let mut extras: BTreeMap<&PackageId, Extras> = BTreeMap::new();
    let mut notices: BTreeMap<String, String> = BTreeMap::new();
    for id in &deps {
        let Some(&pkg) = pkg_of.get(id) else { continue };
        let x = harvest_extras(pkg);
        if let Some(n) = &x.notice {
            // two packages can share a name-version stem (a registry dep plus a git
            // fork); ship both bodies under the one file instead of dropping one.
            use std::collections::btree_map::Entry;
            match notices.entry(format!("{}-{}", pkg.name, pkg.version)) {
                Entry::Vacant(v) => {
                    v.insert(n.clone());
                }
                Entry::Occupied(mut o) => {
                    if o.get() != n {
                        o.get_mut().push('\n');
                        o.get_mut().push_str(n);
                    }
                }
            }
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
            Format::Json => Ok(Output::Report(render_json(&res)?)),
            Format::Text => Ok(Output::Report(render_text(&res, &texts))),
            Format::CycloneDx => Ok(Output::Report(render_cyclonedx(&res, &texts)?)),
        };
    }
    // which artifacts this run writes and checks, per the layout preset.
    let folders = matches!(set.layout, Layout::Folders | Layout::Both);
    let flat = matches!(set.layout, Layout::Flat | Layout::Both);
    let manifest = folders.then(|| render_manifest(&res, &set.licenses_link, &set.notices_link));
    let flat_doc = flat.then(|| render_text(&res, &texts));

    if cli.check {
        let mut stale = Vec::new();
        if let Some(m) = &manifest {
            stale = stale_outputs(&set.licenses_dir, &texts, &set.notices_dir, &notices, &set.manifest, m);
        }
        if let Some(doc) = &flat_doc
            && let Some(entry) = stale_doc(&set.flat_file, doc)
        {
            stale.push(entry);
        }
        if !stale.is_empty() {
            return Err(Failure::Stale(format!("out of date (run `cargo tribute`):\n  {}", stale.join("\n  "))));
        }
        let n = if notices.is_empty() { String::new() } else { format!(", {} notices", notices.len()) };
        let e = if extra_chosen.is_empty() { String::new() } else { format!(", {} extras", extra_chosen.len()) };
        Ok(Output::Summary(format!("up to date: {} license texts{n}, {} crates{e}", texts.len(), deps.len())))
    } else if let Some(manifest) = &manifest {
        // the write path covers the whole workspace (a scoped -p run is report-only),
        // so orphan cleanup over the shared folders is always safe here.
        // drop license/exception texts cargo-tribute wrote that are no longer used; leave other files
        if let Ok(entries) = fs::read_dir(&set.licenses_dir) {
            for e in entries.flatten() {
                let p = e.path();
                if is_stale_license(&p, &texts) {
                    let _ = fs::remove_file(p);
                }
            }
        }
        if texts.is_empty() {
            // nothing to ship: don't leave an empty folder behind (only removes empty).
            let _ = fs::remove_dir(&set.licenses_dir);
        } else {
            io(&set.licenses_dir, fs::create_dir_all(&set.licenses_dir))?;
            for (id, text) in &texts {
                let p = set.licenses_dir.join(format!("{id}.txt"));
                io(&p, fs::write(&p, text))?;
            }
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
        io(&set.manifest, fs::write(&set.manifest, manifest))?;
        if let Some(doc) = &flat_doc {
            if let Some(parent) = set.flat_file.parent() {
                io(parent, fs::create_dir_all(parent))?;
            }
            io(&set.flat_file, fs::write(&set.flat_file, doc))?;
        }
        let n = if notices.is_empty() {
            String::new()
        } else {
            format!(", {}/ ({} notices)", set.notices_link, notices.len())
        };
        let e = if extra_chosen.is_empty() { String::new() } else { format!(", {} extras", extra_chosen.len()) };
        let f = if flat { format!(" and {}", set.flat_link) } else { String::new() };
        Ok(Output::Summary(format!(
            "wrote {}/ ({} license texts){n} and {} ({} crates{e}){f}",
            set.licenses_link,
            texts.len(),
            set.manifest_link,
            deps.len()
        )))
    } else {
        // layout = "flat": the one document is the whole output.
        let doc = flat_doc.as_deref().unwrap_or_default();
        if let Some(parent) = set.flat_file.parent() {
            io(parent, fs::create_dir_all(parent))?;
        }
        io(&set.flat_file, fs::write(&set.flat_file, doc))?;
        let e = if extra_chosen.is_empty() { String::new() } else { format!(", {} extras", extra_chosen.len()) };
        Ok(Output::Summary(format!("wrote {} ({} crates{e})", set.flat_link, deps.len())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_and_init_template_cover_every_config_key() {
        // --help and the init template are hand-kept copies of the config surface;
        // add new tribute.toml keys to this list and both texts, or this fails.
        const KEYS: &[&str] = &[
            "accepted",
            "include-dev",
            "include-build",
            "skip-private",
            "skip-proc-macros",
            "layout",
            "flat-file",
            "manifest",
            "licenses-dir",
            "notices-dir",
            "[[clarify]]",
            "[[exception]]",
            "[[extra]]",
            "[[license-text]]",
        ];
        for k in KEYS {
            assert!(AFTER_HELP.contains(k), "--help lost the config key {k}");
            assert!(INIT_TEMPLATE.contains(k), "the init template lost the config key {k}");
        }
    }

    #[test]
    fn clap_definition_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
