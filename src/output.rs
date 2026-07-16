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
// only a stem that splits into "<name>-<version>" is ours; anything else is hand-added.
pub fn is_stale_notice(path: &Path, notices: &BTreeMap<String, String>) -> bool {
    path.extension().is_some_and(|x| x == "txt")
        && path
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|stem| notice_stem(stem) && !notices.contains_key(stem))
}

// the stem is "<name>-<version>"; a semver version may itself contain '-' (pre-release),
// so accept any '-' split whose suffix parses as a version, not just the last one.
// a bare-word pre-release ("1.0.0-agenda") is more likely a hand-added file, so a
// pre-release only counts with a digit ("-rc.1"); a digitless one lingers instead.
fn notice_stem(stem: &str) -> bool {
    stem.match_indices('-').any(|(i, _)| {
        Version::parse(&stem[i + 1..])
            .is_ok_and(|v| v.pre.is_empty() || v.pre.as_str().chars().any(|c| c.is_ascii_digit()))
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
            let x = r.extras.get(*id);
            Some(CrateEntry {
                name: pkg.name.as_ref(),
                version: pkg.version.to_string(),
                expression: r.effective.get(*id).copied().unwrap_or(""),
                // a policy-failed crate still appears, with no resolved licenses.
                licenses: r.chosen_of.get(*id).map(|c| c.iter().map(String::as_str).collect()).unwrap_or_default(),
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
// pass while stale files still sit in the tree. --check always covers the whole
// workspace (a scoped -p run is report-only), so orphan scanning is unconditional.
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
    if let Some(entry) = stale_doc(manifest_path, manifest) {
        stale.push(entry);
    }
    stale
}

// a written document (the manifest or the flat notices file) that drifted, pointing
// at the first differing line; the files are too big to hand-diff on a failed --check.
pub fn stale_doc(path: &Path, want: &str) -> Option<String> {
    let disk = fs::read_to_string(path).ok();
    if matches_output(disk.clone(), want) {
        return None;
    }
    Some(match &disk {
        Some(d) => {
            let d = d.replace("\r\n", "\n");
            let line = d
                .lines()
                .zip(want.lines())
                .position(|(a, b)| a != b)
                .map_or_else(|| d.lines().count().min(want.lines().count()) + 1, |i| i + 1);
            format!("{} (first difference at line {line})", path.display())
        }
        None => path.display().to_string(),
    })
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

// one flat all-in-one THIRD-PARTY-NOTICES document: part I is a self-contained
// entry per package (source, chosen license beside the upstream expression,
// copyright, NOTICE body), part II every referenced license text once. plain
// ascii, deterministic.
pub fn render_text(r: &Resolution, texts: &BTreeMap<&str, String>) -> String {
    let heavy = "=".repeat(80);
    let light = "-".repeat(80);
    let hash = "#".repeat(80);
    let mut out = String::from("THIRD-PARTY NOTICES\n\nGenerated by cargo tribute; do not edit.\n");
    out.push_str(&format!("\n{heavy}\nPART I -- PER-PACKAGE ENTRIES\n{heavy}\n"));

    // entries sorted by name and version; the deps set orders by opaque package id.
    let mut pkgs: Vec<&Package> = r.deps.iter().filter_map(|id| r.pkg_of.get(id).copied()).collect();
    pkgs.sort_by(|a, b| (&*a.name, &a.version).cmp(&(&*b.name, &b.version)));
    for p in pkgs {
        out.push_str(&format!("\n{light}\n{} {}\n{light}\n", p.name, p.version));
        let url = p.repository.clone().unwrap_or_else(|| format!("https://crates.io/crates/{}", p.name));
        out.push_str(&format!("Source:  {url}\n"));
        let chosen = r.chosen_of.get(&p.id);
        let lic = chosen.map(|c| c.iter().cloned().collect::<Vec<_>>().join(" AND ")).unwrap_or_default();
        push_license_line(&mut out, &lic, r.effective.get(&p.id).copied().unwrap_or(""));
        let x = r.extras.get(&p.id);
        let copyrights = x.map(|x| x.copyrights.as_slice()).unwrap_or(&[]);
        let authors = x.map(|x| x.authors.as_slice()).unwrap_or(&[]);
        // harvested copyright lines, or the declared authors when a crate ships none.
        let holders = if copyrights.is_empty() { authors } else { copyrights };
        if !holders.is_empty() {
            out.push_str("\nCopyright notice:\n");
            for l in holders {
                out.push_str(&format!("  {l}\n"));
            }
        }
        push_text_pointer(&mut out, chosen);
        if let Some(n) = x.and_then(|x| x.notice.as_deref()) {
            out.push_str("\nNOTICE (reproduced as shipped):\n");
            push_indented(&mut out, n);
        }
    }

    if !r.extra_chosen.is_empty() {
        out.push_str(&format!("\n{heavy}\nPART I (continued) -- NON-CRATE CODE\n{heavy}\n"));
        for (x, chosen) in r.extra_chosen {
            out.push_str(&format!("\n{light}\n{}\n{light}\n", x.name));
            if let Some(u) = &x.url {
                out.push_str(&format!("Source:  {u}\n"));
            }
            let lic = chosen.iter().cloned().collect::<Vec<_>>().join(" AND ");
            push_license_line(&mut out, &lic, &x.expression);
            if let Some(c) = &x.copyright {
                out.push_str("\nCopyright notice:\n");
                push_indented(&mut out, c);
            }
            push_text_pointer(&mut out, Some(chosen));
            if let Some(n) = &x.notes {
                out.push_str("\nAdditional requirements / notices:\n");
                push_indented(&mut out, n);
            }
        }
    }

    out.push_str(&format!(
        "\n{heavy}\nPART II -- LICENSE TEXTS\n{heavy}\n\nOriginal license texts referenced by Part I; one copy per license.\n"
    ));
    for (id, text) in texts {
        let tag = if r.used_exceptions.contains(*id) { " (license exception)" } else { "" };
        out.push_str(&format!("\n{hash}\n# {id}{tag}\n{hash}\n\n"));
        out.push_str(text);
        if !text.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

// "License: MIT  (upstream declares: MIT OR Apache-2.0)"; the parenthetical only
// when the upstream expression differs from what we chose.
fn push_license_line(out: &mut String, chosen: &str, upstream: &str) {
    if upstream != chosen && !upstream.is_empty() {
        out.push_str(&format!("License: {chosen}  (upstream declares: {upstream})\n"));
    } else {
        out.push_str(&format!("License: {chosen}\n"));
    }
}

fn push_text_pointer(out: &mut String, chosen: Option<&BTreeSet<String>>) {
    if let Some(c) = chosen
        && !c.is_empty()
    {
        let ids = c.iter().cloned().collect::<Vec<_>>().join(", ");
        out.push_str(&format!("\nLicense text: see Part II -- {ids}\n"));
    }
}

// append text with every line indented two spaces.
fn push_indented(out: &mut String, text: &str) {
    for line in text.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
    }
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
        let Some(&pkg) = r.pkg_of.get(id) else { continue };
        let mut comp = serde_json::json!({
            "type": "library",
            "name": pkg.name.as_ref() as &str,
            "version": pkg.version.to_string(),
        });
        // a policy-failed crate still appears, just without resolved licenses.
        if let Some(chosen) = r.chosen_of.get(id) {
            comp["licenses"] = cdx_licenses(chosen, texts);
        }
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
        // a pre-release version carries its own '-'; still recognized as ours.
        assert!(is_stale_notice(Path::new("N/dep-1.0.0-rc.1.txt"), &notices));
        // a bare-word pre-release is treated as hand-added, not deleted.
        assert!(!is_stale_notice(Path::new("N/meeting-1.0.0-agenda.txt"), &notices));
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
