//! --audit: compare each crate's declared license against the license files it
//! actually ships (spdx text detection). advisory only, never gates.

use crate::harvest::attribution_files;
use crate::policy::license_name;
use cargo_metadata::{Package, PackageId};
use spdx::detection::{Store, TextData};
use spdx::expression::ExprNode;
use std::collections::{BTreeMap, BTreeSet};

// below this, a file just doesn't look like any one license; no point reporting it.
const THRESHOLD: f32 = 0.9;

// findings: license files whose best corpus match is not covered by the crate's
// declared expression (the av1-grain case: declared BSD-2-Clause, ships more).
pub fn run_audit(
    deps: &BTreeSet<&PackageId>,
    pkg_of: &BTreeMap<&PackageId, &Package>,
    effective: &BTreeMap<&PackageId, &str>,
) -> String {
    // one matcher over the whole bundled corpus; built per run, only under --audit.
    let mut store = Store::new();
    for (id, text) in spdx::text::LICENSE_TEXTS {
        store.add_license((*id).into(), TextData::new(text));
    }
    let mut findings = Vec::new();
    let mut scanned = 0usize;
    for id in deps {
        let Some(&pkg) = pkg_of.get(id) else { continue };
        let declared = effective.get(id).copied();
        // every leaf of the declared expression counts as covered, not just the
        // OR-pick: the audit is about what the crate ships, not our preference.
        let leaves: BTreeSet<String> = declared
            .and_then(|e| spdx::Expression::parse_mode(e, spdx::ParseMode::LAX).ok())
            .map(|expr| {
                expr.iter()
                    .filter_map(|n| match n {
                        ExprNode::Req(r) => Some(license_name(&r.req)),
                        ExprNode::Op(_) => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (fname, text, is_notice) in attribution_files(pkg) {
            if is_notice {
                continue;
            }
            scanned += 1;
            let data = TextData::new(&text);
            let m = store.analyze(&data);
            if m.score < THRESHOLD || leaves.contains(m.name) {
                continue;
            }
            // near-identical corpus texts (Apache-2.0 vs Pixar) make the best match a
            // coin toss; if a declared license scores about as well, don't report.
            let declared_close = leaves
                .iter()
                .any(|l| store.get_original(l).is_some_and(|orig| data.match_score(orig) >= m.score - 0.05));
            if declared_close {
                continue;
            }
            findings.push(format!(
                "{} {}: {fname} matches {} (score {:.2}) but the declared license is '{}'",
                pkg.name,
                pkg.version,
                m.name,
                m.score,
                declared.unwrap_or("<none>")
            ));
        }
    }
    if findings.is_empty() {
        format!("audit: no mismatches ({} license files scanned across {} crates)", scanned, deps.len())
    } else {
        format!("audit: {} possible mismatches (advisory only):\n  {}", findings.len(), findings.join("\n  "))
    }
}
