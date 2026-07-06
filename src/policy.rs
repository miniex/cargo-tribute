//! SPDX expression evaluation against the accepted set.

use crate::config::Accept;
use spdx::expression::{ExprNode, Operator};
use std::collections::BTreeSet;

// canonical text for a license or exception id, from the spdx crate's bundled corpus
// (the `text` feature). covers every SPDX id, so no texts are hand-maintained here.
pub fn canonical_text(id: &str) -> Option<&'static str> {
    spdx::license_id(id).map(|l| l.text()).or_else(|| spdx::exception_id(id).map(|e| e.text()))
}

// SPDX exception ids (from `A WITH exception`) attached to a license we actually chose, so
// their text ships too. a WITH on a license the OR-pick dropped is not attributed.
pub fn exceptions_for(expr: &spdx::Expression, chosen: &BTreeSet<String>) -> Vec<String> {
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
pub fn choose(expr: &spdx::Expression, accepted: &[Accept]) -> Option<BTreeSet<String>> {
    let mut stack: Vec<Option<BTreeSet<String>>> = Vec::new();
    for node in expr.iter() {
        match node {
            ExprNode::Req(req) => {
                // a leaf is `license` or `license WITH exception`; see Accept::allows.
                let ex = req.req.addition.as_ref().and_then(|a| a.id()).map(|e| e.name);
                let leaf = req
                    .req
                    .license
                    .id()
                    .map(|id| id.name)
                    .filter(|n| accepted.iter().any(|a| a.allows(n, ex)))
                    .map(|n| BTreeSet::from([n.to_string()]));
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
    accepted: &[Accept],
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

fn best(set: &BTreeSet<String>, accepted: &[Accept]) -> usize {
    set.iter().map(|l| accepted.iter().position(|a| a.license == *l).unwrap_or(usize::MAX)).min().unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DEFAULT_ACCEPTED, parse_accept};

    fn pick_with(accepted: &[&str], s: &str) -> Option<Vec<String>> {
        let acc: Vec<Accept> = accepted.iter().map(|s| parse_accept(s)).collect();
        let e = spdx::Expression::parse_mode(s, spdx::ParseMode::LAX).unwrap();
        choose(&e, &acc).map(|set| set.into_iter().collect())
    }

    fn pick(s: &str) -> Option<Vec<String>> {
        pick_with(DEFAULT_ACCEPTED, s)
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
    fn accepted_with_pairing_allows_only_that_pairing() {
        let acc = &["MIT", "GPL-2.0-only WITH Classpath-exception-2.0"];
        assert_eq!(pick_with(acc, "GPL-2.0-only WITH Classpath-exception-2.0"), Some(vec!["GPL-2.0-only".into()]));
        assert_eq!(pick_with(acc, "GPL-2.0-only"), None); // the pairing does not allow the bare license
        assert_eq!(pick_with(acc, "GPL-2.0-only WITH GCC-exception-2.0"), None); // nor another exception
        // preference still works: MIT (earlier) beats the pairing in an OR.
        assert_eq!(pick_with(acc, "(GPL-2.0-only WITH Classpath-exception-2.0) OR MIT"), Some(vec!["MIT".into()]));
    }

    #[test]
    fn exception_allow_extends_accepted_per_crate() {
        // [[exception]] appends its allow list after the global accepted set; simulate
        // the per-crate composition run() does.
        let acc = &["MIT", "MPL-2.0"];
        assert_eq!(pick_with(acc, "MPL-2.0"), Some(vec!["MPL-2.0".into()]));
        assert_eq!(pick_with(&["MIT"], "MPL-2.0"), None); // without it, rejected
        // appended entries lose the OR preference to global ones.
        assert_eq!(pick_with(acc, "MPL-2.0 OR MIT"), Some(vec!["MIT".into()]));
    }
}
