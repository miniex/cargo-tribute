//! tribute.toml -- the accepted-license policy, per-crate overrides, and output paths.

use cargo_metadata::camino::Utf8Path;
use cargo_metadata::semver::{Version, VersionReq};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

// default allowed licenses; also the OR preference order (earlier wins when an
// "A OR B" can pick either). Overridable via tribute.toml.
pub const DEFAULT_ACCEPTED: &[&str] =
    &["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "0BSD", "Zlib", "Unlicense", "Unicode-3.0"];

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Config {
    accepted: Option<Vec<String>>,
    #[serde(rename = "include-dev")]
    include_dev: Option<bool>,
    #[serde(rename = "include-build")]
    include_build: Option<bool>,
    #[serde(rename = "skip-private")]
    skip_private: Option<bool>,
    #[serde(rename = "skip-proc-macros")]
    skip_proc_macros: Option<bool>,
    manifest: Option<String>,
    #[serde(rename = "licenses-dir")]
    licenses_dir: Option<String>,
    #[serde(rename = "notices-dir")]
    notices_dir: Option<String>,
    layout: Option<String>,
    #[serde(rename = "flat-file")]
    flat_file: Option<String>,
    clarify: Option<Vec<Clarify>>,
    exception: Option<Vec<Exception>>,
    extra: Option<Vec<Extra>>,
    #[serde(rename = "license-text")]
    license_text: Option<Vec<LicenseText>>,
}

// override a crate's license when its `license` field is missing (crates that use
// `license-file` instead), wrong, or non-SPDX. `version` optional: omit to match any.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clarify {
    pub name: String,
    pub version: Option<String>,
    pub expression: String,
}

// allow extra licenses for one crate only, without widening the global accepted set.
// `version` optional, like [[clarify]].
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exception {
    pub name: String,
    pub version: Option<String>,
    pub allow: Vec<String>,
}

// attribute third-party code the crate graph can't see (C sources vendored in a
// -sys crate, a bundled font, ...); the expression flows through the same policy.
// `notes` is free text reproduced in the notices file (provenance, vendored
// paths, 4(b) change notices).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Extra {
    pub name: String,
    pub expression: String,
    pub url: Option<String>,
    pub copyright: Option<String>,
    pub notes: Option<String>,
}

// a local text file for a license id outside the SPDX corpus (LicenseRef-<id>),
// written into the licenses dir like a canonical text.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LicenseText {
    pub id: String,
    pub file: String,
}

// an accepted-list entry: a bare "MIT" allows the license with or without an
// exception; a "GPL-2.0-only WITH Classpath-exception-2.0" only that exact pairing.
#[derive(Clone)]
pub struct Accept {
    pub raw: String,
    pub license: String,
    pub exception: Option<String>,
}

// byte offset of a case-insensitive " WITH " separator; SPDX spells it either case,
// and LAX parsing accepts both, so the accepted list should too.
fn find_with(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    (0..b.len().saturating_sub(5)).find(|&i| b[i..i + 6].eq_ignore_ascii_case(b" with "))
}

pub fn parse_accept(s: &str) -> Accept {
    match find_with(s) {
        Some(i) => Accept { raw: s.into(), license: s[..i].trim().into(), exception: Some(s[i + 6..].trim().into()) },
        None => Accept { raw: s.into(), license: s.trim().into(), exception: None },
    }
}

impl Accept {
    // does this entry allow a leaf `license` (optionally `WITH exception`)?
    pub fn allows(&self, license: &str, exception: Option<&str>) -> bool {
        self.license == license && self.exception.as_deref().is_none_or(|e| exception == Some(e))
    }
}

pub struct Settings {
    pub accepted: Vec<Accept>,
    pub accepted_explicit: bool, // came from tribute.toml, not the built-in default
    pub include_dev: bool,
    pub include_build: bool,
    pub skip_private: bool,     // skip deps not from crates.io (path/git/alt registry)
    pub skip_proc_macros: bool, // skip proc-macro crates and their compile-time subtree
    pub clarify: Vec<Clarify>,
    pub exception: Vec<Exception>,
    pub extra: Vec<Extra>,
    pub license_text: Vec<LicenseText>,
    pub manifest: PathBuf,     // absolute output path
    pub manifest_link: String, // relative name, for messages
    pub licenses_dir: PathBuf, // absolute output dir
    pub licenses_link: String, // relative name, for markdown links + messages
    pub notices_dir: PathBuf,  // absolute output dir for NOTICE files
    pub notices_link: String,  // relative name, for markdown links + messages
    pub layout: Layout,
    pub flat_file: PathBuf, // absolute path of the flat document (layout flat/both)
    pub flat_link: String,  // relative name, for messages
}

// which artifacts a run writes and --check gates.
#[derive(Clone, Copy, PartialEq)]
pub enum Layout {
    Folders, // LICENSES/ + NOTICES/ + THIRD-PARTY.md (the default)
    Flat,    // one flat all-in-one THIRD-PARTY-NOTICES file
    Both,
}

// anchor tribute.toml and outputs to the workspace root, not the cwd, so
// --manifest-path against a crate elsewhere reads and writes beside that crate.
pub fn load_settings(root: &Utf8Path) -> Result<Settings, String> {
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
    let flat_link = cfg.flat_file.unwrap_or_else(|| "THIRD-PARTY-NOTICES".into());
    relative_inside("manifest", &manifest_link)?;
    relative_inside("licenses-dir", &licenses_link)?;
    relative_inside("notices-dir", &notices_link)?;
    relative_inside("flat-file", &flat_link)?;
    // the same path for both documents would self-overwrite under layout = "both"
    // and leave --check permanently stale.
    if flat_link == manifest_link {
        return Err(format!("tribute.toml: manifest and flat-file point at the same path ('{flat_link}')"));
    }
    let layout = match cfg.layout.as_deref() {
        None | Some("folders") => Layout::Folders,
        Some("flat") => Layout::Flat,
        Some("both") => Layout::Both,
        Some(other) => {
            return Err(format!("tribute.toml: layout must be \"folders\", \"flat\", or \"both\" (got '{other}')"));
        }
    };
    // license-text files are only read, but keep them inside the project anyway so
    // the output cannot depend on files outside the tree.
    for t in cfg.license_text.as_deref().unwrap_or_default() {
        relative_inside("license-text file", &t.file)?;
    }
    let accepted_explicit = cfg.accepted.is_some();
    let accepted = cfg
        .accepted
        .unwrap_or_else(|| DEFAULT_ACCEPTED.iter().map(|s| s.to_string()).collect())
        .iter()
        .map(|s| parse_accept(s))
        .collect();
    Ok(Settings {
        accepted,
        accepted_explicit,
        include_dev: cfg.include_dev.unwrap_or(false),
        include_build: cfg.include_build.unwrap_or(false),
        skip_private: cfg.skip_private.unwrap_or(false),
        skip_proc_macros: cfg.skip_proc_macros.unwrap_or(false),
        clarify: cfg.clarify.unwrap_or_default(),
        exception: cfg.exception.unwrap_or_default(),
        extra: cfg.extra.unwrap_or_default(),
        license_text: cfg.license_text.unwrap_or_default(),
        manifest: root.join(&manifest_link).into(),
        licenses_dir: root.join(&licenses_link).into(),
        notices_dir: root.join(&notices_link).into(),
        flat_file: root.join(&flat_link).into(),
        layout,
        manifest_link,
        licenses_link,
        notices_link,
        flat_link,
    })
}

// reject a config output path that is absolute, escapes the project via `..`, or names
// no real target (empty or "."). the last would resolve to the project root itself, so
// orphan-cleanup (which deletes bundled-id `.txt`s) would then scan the whole tree.
// has_root, not is_absolute: on windows a rooted "\evil" or drive-relative "C:evil"
// is not "absolute", but join() still leaves the project with it.
fn relative_inside(field: &str, link: &str) -> Result<(), String> {
    use std::path::Component;
    let p = Path::new(link);
    let escapes = p.has_root() || p.components().any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)));
    let has_target = p.components().any(|c| matches!(c, Component::Normal(_)));
    if escapes || !has_target {
        return Err(format!("tribute.toml: {field} must be a relative path inside the project (got '{link}')"));
    }
    Ok(())
}

// a clarify/exception entry applies to this crate: name equal, and if it gives a version
// it parses as a semver requirement the crate satisfies (so "1.0" matches 1.0.0, like Cargo).
pub fn policy_matches(name: &str, version_req: Option<&str>, pkg: &str, version: &Version) -> bool {
    name == pkg && version_req.is_none_or(|v| VersionReq::parse(v).is_ok_and(|req| req.matches(version)))
}

fn clarify_matches(c: &Clarify, name: &str, version: &Version) -> bool {
    policy_matches(&c.name, c.version.as_deref(), name, version)
}

// warn when a policy entry is not a known SPDX id. a LicenseRef-* name is not in
// the corpus by design (its text comes from [[license-text]]), so it never warns.
pub fn warn_unknown_ids(kind: &str, a: &Accept) {
    if !a.license.starts_with("LicenseRef-") && spdx::license_id(&a.license).is_none() {
        eprintln!("cargo-tribute: warning: {kind} license '{}' is not a known SPDX id", a.license);
    }
    if let Some(e) = &a.exception
        && spdx::exception_id(e).is_none()
    {
        eprintln!("cargo-tribute: warning: {kind} exception '{e}' is not a known SPDX id");
    }
}

// a tribute.toml [[clarify]] expression overriding this crate's declared license.
pub fn clarify_expr<'a>(clarify: &'a [Clarify], name: &str, version: &Version) -> Option<&'a str> {
    clarify.iter().find(|c| clarify_matches(c, name, version)).map(|c| c.expression.as_str())
}

// the subset of a cargo-deny deny.toml we can reuse; everything else in that file
// is cargo-deny's business, so no deny_unknown_fields here.
#[derive(Deserialize)]
struct DenyConfig {
    licenses: Option<DenyLicenses>,
}

#[derive(Deserialize)]
struct DenyLicenses {
    allow: Option<Vec<String>>,
    exceptions: Option<Vec<DenyException>>,
}

#[derive(Deserialize)]
struct DenyException {
    // cargo-deny spells it `crate` today and `name` historically; take both.
    #[serde(rename = "crate", alias = "name")]
    name: String,
    allow: Vec<String>,
    version: Option<String>,
}

// take the accepted list (and per-crate exceptions) from deny.toml's [licenses],
// so teams already on cargo-deny keep the allowlist in one place.
pub fn apply_deny(set: &mut Settings, path: &Path) -> Result<(), String> {
    if set.accepted_explicit {
        return Err("tribute.toml sets `accepted` and --from-deny is given; keep one source".into());
    }
    let s = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let cfg: DenyConfig = toml::from_str(&s).map_err(|e| format!("{}: {e}", path.display()))?;
    let lic = cfg.licenses.ok_or_else(|| format!("{}: no [licenses] section", path.display()))?;
    let allow = lic.allow.ok_or_else(|| format!("{}: no [licenses] allow list", path.display()))?;
    set.accepted = allow.iter().map(|s| parse_accept(s)).collect();
    set.accepted_explicit = true;
    for e in lic.exceptions.unwrap_or_default() {
        set.exception.push(Exception { name: e.name, version: e.version, allow: e.allow });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn parse_accept_splits_with_either_case() {
        let a = parse_accept("GPL-2.0-only WITH Classpath-exception-2.0");
        assert_eq!((a.license.as_str(), a.exception.as_deref()), ("GPL-2.0-only", Some("Classpath-exception-2.0")));
        // LAX spells WITH in any case; the accepted list follows.
        let b = parse_accept("GPL-2.0-only with Classpath-exception-2.0");
        assert_eq!((b.license.as_str(), b.exception.as_deref()), ("GPL-2.0-only", Some("Classpath-exception-2.0")));
        let c = parse_accept("MIT");
        assert_eq!((c.license.as_str(), c.exception.as_deref()), ("MIT", None));
    }

    #[test]
    fn relative_inside_rejects_escapes_and_rootlike() {
        assert!(relative_inside("manifest", "THIRD-PARTY.md").is_ok());
        assert!(relative_inside("licenses-dir", "docs/LICENSES").is_ok());
        assert!(relative_inside("manifest", "").is_err()); // no target -> project root
        assert!(relative_inside("licenses-dir", ".").is_err()); // "." -> project root
        assert!(relative_inside("manifest", "../escape.md").is_err());
        assert!(relative_inside("manifest", "/etc/passwd").is_err()); // rooted on every platform
        // windows-only shapes: rooted without a drive, and drive prefixes; join()
        // leaves the project with any of them.
        #[cfg(windows)]
        {
            assert!(relative_inside("manifest", r"\evil.md").is_err());
            assert!(relative_inside("manifest", r"C:\evil.md").is_err());
            assert!(relative_inside("manifest", r"C:evil.md").is_err());
        }
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

        // manifest and flat-file on the same path would self-overwrite under "both".
        fs::write(dir.join("tribute.toml"), "manifest = \"NOTICES.txt\"\nflat-file = \"NOTICES.txt\"\n").unwrap();
        let err = load_settings(&root).err().expect("same-path config must be rejected");
        assert!(err.contains("same path"), "err: {err}");

        fs::remove_dir_all(&dir).ok();
    }
}
