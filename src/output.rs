//! render the attribution (THIRD-PARTY.md, json, text, cyclonedx) and detect stale
//! outputs for --check.

use crate::config::Extra;
use crate::harvest::{Extras, display_author};
use crate::policy::canonical_text;
use cargo_metadata::semver::Version;
use cargo_metadata::{Package, PackageId};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

// wrap an io result with the path, so a failure names the file instead of a bare errno.
pub fn io<T>(path: &Path, r: std::io::Result<T>) -> Result<T, String> {
    r.map_err(|e| format!("{}: {e}", path.display()))
}

// a LICENSES/<id>.txt cargo-tribute could write that is no longer used: the stem is
// an SPDX license/exception id, or a LicenseRef-* copied from [[license-text]]. a
// .txt with any other stem is hand-added and left alone.
pub fn is_stale_license(path: &Path, texts: &BTreeMap<&str, String>) -> bool {
    path.extension().is_some_and(|x| x == "txt")
        && path
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|s| (canonical_text(s).is_some() || s.starts_with("LicenseRef-")) && !texts.contains_key(s))
}

// a NOTICES/<name>-<version>.txt cargo-tribute could write that is no longer used.
// only a stem ending in "-<semver>" is ours; anything else is hand-added and left alone.
pub fn is_stale_notice(path: &Path, notices: &BTreeMap<String, String>) -> bool {
    path.extension().is_some_and(|x| x == "txt")
        && path.file_stem().and_then(|s| s.to_str()).is_some_and(|stem| {
            stem.rsplit_once('-').is_some_and(|(_, v)| Version::parse(v).is_ok()) && !notices.contains_key(stem)
        })
}

// everything the resolve pass produced, in one place for the renderers.
pub struct Resolution<'a> {
    pub deps: &'a BTreeSet<&'a PackageId>,
    pub pkg_of: &'a BTreeMap<&'a PackageId, &'a Package>,
    pub effective: &'a BTreeMap<&'a PackageId, &'a str>,
    pub chosen_of: &'a BTreeMap<&'a PackageId, BTreeSet<String>>,
    pub by_license: &'a BTreeMap<String, Vec<&'a Package>>,
    pub extra_by_license: &'a BTreeMap<String, Vec<&'a Extra>>,
    pub extra_chosen: &'a [(&'a Extra, BTreeSet<String>)],
    pub used_exceptions: &'a BTreeSet<String>,
    pub extras: &'a BTreeMap<&'a PackageId, Extras>,
}

#[derive(Serialize)]
struct Report<'a> {
    licenses: Vec<&'a str>,   // license ids used, with a text in the LICENSES dir
    exceptions: Vec<&'a str>, // WITH-exception ids used
    crates: Vec<CrateEntry<'a>>,
    extras: Vec<ExtraEntry<'a>>, // [[extra]] entries, attributed like crates
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

#[derive(Serialize)]
struct ExtraEntry<'a> {
    name: &'a str,
    expression: &'a str,
    licenses: Vec<&'a str>, // ids this entry is attributed under
    url: Option<&'a str>,
    copyright: Option<&'a str>,
}

// the resolved attribution as JSON, for audit/pipeline use. read-only: no files touched.
pub fn render_json(r: &Resolution) -> Result<String, String> {
    let crates: Vec<CrateEntry> = r
        .deps
        .iter()
        .filter_map(|id| {
            let pkg = r.pkg_of.get(id).copied()?;
            let chosen = r.chosen_of.get(*id)?;
            let x = r.extras.get(*id);
            Some(CrateEntry {
                name: pkg.name.as_ref(),
                version: pkg.version.to_string(),
                expression: r.effective.get(*id).copied().unwrap_or(""),
                licenses: chosen.iter().map(String::as_str).collect(),
                authors: x.map(|x| x.authors.as_slice()).unwrap_or(&[]),
                copyrights: x.map(|x| x.copyrights.as_slice()).unwrap_or(&[]),
                notice: x.and_then(|x| x.notice.as_deref()),
            })
        })
        .collect();
    let report = Report {
        // crate licenses and [[extra]] licenses alike; BTreeSet dedups and sorts.
        licenses: r
            .by_license
            .keys()
            .chain(r.extra_by_license.keys())
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        exceptions: r.used_exceptions.iter().map(String::as_str).collect(),
        crates,
        extras: r
            .extra_chosen
            .iter()
            .map(|(x, chosen)| ExtraEntry {
                name: &x.name,
                expression: &x.expression,
                licenses: chosen.iter().map(String::as_str).collect(),
                url: x.url.as_deref(),
                copyright: x.copyright.as_deref(),
            })
            .collect(),
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
pub fn stale_outputs(
    licenses_dir: &Path,
    texts: &BTreeMap<&str, String>,
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

pub fn render_manifest(r: &Resolution, licenses_dir: &str, notices_dir: &str) -> String {
    let mut out = String::from(
        "# Third-party licenses\n\nDependencies linked into this crate, grouped by license; full texts are in \
         [`",
    );
    out.push_str(licenses_dir);
    out.push_str("/`](");
    out.push_str(licenses_dir);
    out.push(')');
    // mention the notices folder only when this tree ships one.
    if r.extras.values().any(|x| x.notice.is_some()) {
        out.push_str(&format!(", NOTICE files shipped by dependencies in [`{notices_dir}/`]({notices_dir})"));
    }
    out.push_str(". Generated by `cargo tribute`; do not edit.\n\n");
    // sections cover crate licenses and [[extra]] licenses alike.
    let ids: BTreeSet<&String> = r.by_license.keys().chain(r.extra_by_license.keys()).collect();
    for id in ids {
        let mut ps: Vec<&Package> = r.by_license.get(id).cloned().unwrap_or_default();
        ps.sort_by(|a, b| (&*a.name, &a.version).cmp(&(&*b.name, &b.version)));
        ps.dedup_by(|a, b| a.id == b.id);
        out.push_str(&format!("## {id}\n\nText: [`{licenses_dir}/{id}.txt`]({licenses_dir}/{id}.txt)\n\n"));
        for p in ps {
            let url = p.repository.clone().unwrap_or_else(|| format!("https://crates.io/crates/{}", p.name));
            out.push_str(&format!("- [{} {}]({url})", p.name, p.version));
            // show the effective SPDX (clarified or declared) when it differs from the
            // section license, so WITH exceptions and dual-license picks are not hidden by
            // the grouping. the exception's own text is written to the licenses dir too.
            if let Some(expr) = r.effective.get(&p.id).copied().filter(|e| *e != id.as_str()) {
                out.push_str(&format!(" -- `{expr}`"));
            }
            if let Some(x) = r.extras.get(&p.id) {
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
        // [[extra]] entries attributed under this license, after the crates.
        for x in r.extra_by_license.get(id).into_iter().flatten() {
            match &x.url {
                Some(u) => out.push_str(&format!("- [{}]({u})", x.name)),
                None => out.push_str(&format!("- {}", x.name)),
            }
            if x.expression != *id {
                out.push_str(&format!(" -- `{}`", x.expression));
            }
            if let Some(c) = &x.copyright {
                out.push_str(&format!(" -- {c}"));
            }
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

// one flat plain-text document -- attribution list, license texts, then NOTICE
// bodies -- for an "open source licenses" screen; no markdown.
pub fn render_text(r: &Resolution, texts: &BTreeMap<&str, String>) -> String {
    let sep = "=".repeat(72);
    let mut out = String::from(
        "Third-party licenses\n\nDependencies grouped by license; the full text follows each group. Generated by cargo tribute.\n",
    );
    let ids: BTreeSet<&String> = r.by_license.keys().chain(r.extra_by_license.keys()).collect();
    for id in ids {
        out.push_str(&format!("\n{sep}\n{id}\n{sep}\n\n"));
        let mut ps: Vec<&Package> = r.by_license.get(id).cloned().unwrap_or_default();
        ps.sort_by(|a, b| (&*a.name, &a.version).cmp(&(&*b.name, &b.version)));
        ps.dedup_by(|a, b| a.id == b.id);
        for p in ps {
            let url = p.repository.clone().unwrap_or_else(|| format!("https://crates.io/crates/{}", p.name));
            out.push_str(&format!("- {} {} ({url})", p.name, p.version));
            if let Some(expr) = r.effective.get(&p.id).copied().filter(|e| *e != id.as_str()) {
                out.push_str(&format!(" -- {expr}"));
            }
            if let Some(x) = r.extras.get(&p.id) {
                if !x.copyrights.is_empty() {
                    out.push_str(&format!(" -- {}", x.copyrights.join("; ")));
                } else {
                    let names: Vec<&str> =
                        x.authors.iter().map(|a| display_author(a)).filter(|s| !s.is_empty()).collect();
                    if !names.is_empty() {
                        out.push_str(&format!(" -- by {}", names.join(", ")));
                    }
                }
            }
            out.push('\n');
        }
        for x in r.extra_by_license.get(id).into_iter().flatten() {
            match &x.url {
                Some(u) => out.push_str(&format!("- {} ({u})", x.name)),
                None => out.push_str(&format!("- {}", x.name)),
            }
            if x.expression != *id {
                out.push_str(&format!(" -- {}", x.expression));
            }
            if let Some(c) = &x.copyright {
                out.push_str(&format!(" -- {c}"));
            }
            out.push('\n');
        }
        if let Some(text) = texts.get(id.as_str()) {
            out.push('\n');
            out.push_str(text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    // WITH-exception bodies ship too, like in the licenses dir.
    for ex in r.used_exceptions {
        out.push_str(&format!("\n{sep}\n{ex} (license exception)\n{sep}\n\n"));
        if let Some(text) = texts.get(ex.as_str()) {
            out.push_str(text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    // NOTICE bodies last, so Apache-2.0 4(d) is covered by the one document.
    for id in r.deps {
        let (Some(&pkg), Some(x)) = (r.pkg_of.get(id), r.extras.get(id)) else { continue };
        if let Some(n) = &x.notice {
            out.push_str(&format!("\n{sep}\nNOTICE for {} {}\n{sep}\n\n", pkg.name, pkg.version));
            out.push_str(n);
            if !n.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out
}

// the licenses array for one component: chosen ids with their full text embedded.
// a non-corpus id (LicenseRef-*) goes under "name"; the schema restricts "id" to SPDX.
fn cdx_licenses(chosen: &BTreeSet<String>, texts: &BTreeMap<&str, String>) -> serde_json::Value {
    let list: Vec<serde_json::Value> = chosen
        .iter()
        .map(|id| {
            let key = if canonical_text(id).is_some() { "id" } else { "name" };
            let mut lic = serde_json::json!({ key: id });
            if let Some(t) = texts.get(id.as_str()) {
                lic["text"] = serde_json::json!({ "contentType": "text/plain", "content": t });
            }
            serde_json::json!({ "license": lic })
        })
        .collect();
    serde_json::Value::Array(list)
}

// the crate's copyright notice for the SBOM: harvested lines, or the authors.
fn cdx_copyright(x: &Extras) -> Option<String> {
    if !x.copyrights.is_empty() {
        return Some(x.copyrights.join("; "));
    }
    let names: Vec<&str> = x.authors.iter().map(|a| display_author(a)).filter(|s| !s.is_empty()).collect();
    (!names.is_empty()).then(|| names.join(", "))
}

// CycloneDX 1.6 JSON with license texts and per-component copyright -- the fields
// id-only SBOM generators leave empty. serialNumber and timestamp are omitted on
// purpose: the output must stay deterministic (same tree -> same bytes).
pub fn render_cyclonedx(r: &Resolution, texts: &BTreeMap<&str, String>) -> Result<String, String> {
    let mut components: Vec<serde_json::Value> = Vec::new();
    for id in r.deps {
        let (Some(&pkg), Some(chosen)) = (r.pkg_of.get(id), r.chosen_of.get(id)) else { continue };
        let mut comp = serde_json::json!({
            "type": "library",
            "name": pkg.name.as_ref() as &str,
            "version": pkg.version.to_string(),
            "licenses": cdx_licenses(chosen, texts),
        });
        // a purl names a registry package; a path or git dep has none.
        if pkg.source.as_ref().is_some_and(|s| s.is_crates_io()) {
            comp["purl"] = serde_json::json!(format!("pkg:cargo/{}@{}", pkg.name, pkg.version));
        }
        if let Some(c) = r.extras.get(id).and_then(cdx_copyright) {
            comp["copyright"] = serde_json::json!(c);
        }
        if let Some(repo) = &pkg.repository {
            comp["externalReferences"] = serde_json::json!([{ "type": "vcs", "url": repo }]);
        }
        // the effective SPDX expression; the licenses array only carries the chosen ids.
        if let Some(expr) = r.effective.get(id) {
            comp["properties"] = serde_json::json!([{ "name": "cargo-tribute:expression", "value": expr }]);
        }
        components.push(comp);
    }
    for (x, chosen) in r.extra_chosen {
        let mut comp = serde_json::json!({
            "type": "library",
            "name": x.name,
            "licenses": cdx_licenses(chosen, texts),
        });
        if let Some(c) = &x.copyright {
            comp["copyright"] = serde_json::json!(c);
        }
        if let Some(u) = &x.url {
            comp["externalReferences"] = serde_json::json!([{ "type": "website", "url": u }]);
        }
        comp["properties"] = serde_json::json!([{ "name": "cargo-tribute:expression", "value": x.expression }]);
        components.push(comp);
    }
    let bom = serde_json::json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.6",
        "version": 1,
        "metadata": {
            "tools": {
                "components": [{ "type": "application", "name": "cargo-tribute", "version": env!("CARGO_PKG_VERSION") }]
            }
        },
        "components": components,
    });
    serde_json::to_string_pretty(&bom).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_detects_missing_and_orphan() {
        let dir = std::env::temp_dir().join(format!("tribute-test-{}", std::process::id()));
        let lic = dir.join("LICENSES");
        let not = dir.join("NOTICES");
        fs::create_dir_all(&lic).unwrap();
        fs::create_dir_all(&not).unwrap();
        let manifest_path = dir.join("THIRD-PARTY.md");
        let mut texts: BTreeMap<&str, String> = BTreeMap::new();
        texts.insert("MIT", "MIT TEXT".into());
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

        // an unused LicenseRef-* text is ours (copied from [[license-text]]) -> stale.
        fs::write(lic.join("LicenseRef-old.txt"), "x").unwrap();
        let stale = stale_outputs(&lic, &texts, &not, &notices, &manifest_path, "MANIFEST");
        assert!(stale.iter().any(|s| s.contains("LicenseRef-old.txt")));
        fs::remove_file(lic.join("LicenseRef-old.txt")).unwrap();

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
    fn matches_output_ignores_crlf() {
        // a CRLF checkout (git autocrlf) of an LF-written file is not stale.
        assert!(matches_output(Some("a\r\nb\r\n".into()), "a\nb\n"));
        assert!(matches_output(Some("a\nb\n".into()), "a\nb\n"));
        assert!(!matches_output(Some("a\nb\n".into()), "a\nDIFFERENT\n"));
        assert!(!matches_output(None, "x")); // missing file is stale
    }
}
