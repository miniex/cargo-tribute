//! copyright lines and NOTICE bodies harvested from the local crate sources.

use cargo_metadata::Package;
use std::collections::BTreeSet;
use std::fs;

// per-crate extras from the crate source: copyright lines out of license/notice
// files (authors as a fallback), and the NOTICE body Apache-2.0 4(d) says to pass along.
pub struct Extras {
    pub copyrights: Vec<String>, // "Copyright ..." lines, deduped and sorted
    pub authors: Vec<String>,    // metadata authors, as declared
    pub notice: Option<String>,  // NOTICE file contents, LF-normalized
}

// scan the crate root (already local in cargo's registry cache, so still offline).
// best-effort: an unreadable or huge file is skipped, extras never gate.
pub fn harvest_extras(pkg: &Package) -> Extras {
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
pub fn display_author(a: &str) -> &str {
    match a.split_once('<') {
        Some((name, _)) if !name.trim().is_empty() => name.trim(),
        _ => a.trim(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
