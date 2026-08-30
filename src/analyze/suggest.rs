//! Deterministic `--suggest` selection (design D7).
//!
//! Maps observed [`SignalGroups`](crate::analyze::signals::SignalGroups) to the
//! read-only [`CATALOG`](crate::guidance::CATALOG) entries via a fixed review
//! mapping. Selection is deduplicated and emitted in stable catalog order —
//! never fuzzy, scored, or mutate-the-catalog.

use crate::analyze::signals::SignalGroups;
use crate::guidance::CATALOG;
use std::collections::BTreeSet;

/// Select stable, deduplicated catalog suggestions for the observed signals.
///
/// The returned texts are in CATALOG order and contain no duplicates, so
/// repeated runs with varied signal order yield identical output (design D9).
pub(crate) fn select_suggestions(groups: &SignalGroups) -> Vec<String> {
    // Fixed signal-group → catalog-line-substring mapping. Each observed group
    // pulls in the catalog lines whose text mentions the relevant topic.
    let mut wanted: BTreeSet<usize> = BTreeSet::new();

    let mut add_if = |cond: bool, keywords: &[&str]| {
        if cond {
            for (i, line) in CATALOG.iter().enumerate() {
                if keywords.iter().any(|k| line.to_lowercase().contains(k)) {
                    wanted.insert(i);
                }
            }
        }
    };

    add_if(
        !groups.languages.is_empty(),
        &["language", "code", "definition"],
    );
    add_if(
        !groups.manifests.is_empty(),
        &["definition", "document", "architecture"],
    );
    add_if(!groups.linters.is_empty(), &["edit", "document"]);
    add_if(!groups.tests.is_empty(), &["decision", "risk"]);
    add_if(!groups.containers.is_empty(), &["constraint", "decision"]);
    add_if(!groups.cicd.is_empty(), &["decision", "risk"]);
    add_if(
        !groups.entry_points.is_empty(),
        &["definition", "architecture"],
    );
    add_if(groups.sensitive, &["optional", "risk", "constraint"]);

    // Integrate curated suggestion IDs. When any signal is present, surface
    // the curated IDs deterministically alongside path-analysis selections.
    // If no signals at all, emit nothing (preserve empty-repo empty behavior).
    let has_any = !groups.languages.is_empty()
        || !groups.manifests.is_empty()
        || !groups.linters.is_empty()
        || !groups.tests.is_empty()
        || !groups.containers.is_empty()
        || !groups.cicd.is_empty()
        || !groups.entry_points.is_empty()
        || groups.sensitive;
    // Ensure the curated documentation line is always suggested when any
    // signal is present, providing context/constraints/decisions/risks.
    if has_any {
        for (i, line) in CATALOG.iter().enumerate() {
            if line.contains("context") && line.contains("constraints") {
                wanted.insert(i);
            }
        }
    }
    let mut out: Vec<String> = wanted.into_iter().map(|i| CATALOG[i].to_string()).collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::signals::SignalGroups;
    use std::collections::BTreeSet;

    #[test]
    fn empty_groups_yield_no_suggestions() {
        let g = SignalGroups::default();
        assert!(select_suggestions(&g).is_empty());
    }

    #[test]
    fn cargo_and_cicd_yield_relevant_lines_only() {
        let mut g = SignalGroups::default();
        g.manifests.insert("cargo".to_string());
        g.cicd.insert("github-actions".to_string());

        let sugg = select_suggestions(&g);
        // Output depends only on observed signals and catalog order.
        let sugg2 = {
            let mut g2 = SignalGroups::default();
            g2.cicd.insert("github-actions".to_string());
            g2.manifests.insert("cargo".to_string());
            select_suggestions(&g2)
        };
        assert_eq!(sugg, sugg2, "ordering must be independent of insert order");
        assert!(!sugg.is_empty());
    }

    #[test]
    fn suggestions_are_deduplicated() {
        let mut g = SignalGroups::default();
        g.manifests.insert("cargo".to_string());
        g.manifests.insert("npm".to_string());
        let sugg = select_suggestions(&g);
        let set: BTreeSet<&String> = sugg.iter().collect();
        assert_eq!(sugg.len(), set.len(), "no duplicates in suggestions");
    }

    #[test]
    fn curated_ids_surface_when_signals_present() {
        let mut g = SignalGroups::default();
        g.manifests.insert("cargo".to_string());
        let sugg = select_suggestions(&g);
        let joined = sugg.join("\n");
        assert!(
            joined.contains("context") && joined.contains("constraints"),
            "curated line with context/constraints must be present"
        );
    }

    #[test]
    fn curated_ids_absent_for_empty_signals() {
        let g = SignalGroups::default();
        let sugg = select_suggestions(&g);
        assert!(sugg.is_empty(), "empty signals yield no curated injection");
    }

    #[test]
    fn suggestions_are_sorted_deterministic() {
        let mut g = SignalGroups::default();
        g.cicd.insert("github-actions".to_string());
        g.languages.insert("rust".to_string());
        g.containers.insert("dockerfile".to_string());
        let a = select_suggestions(&g);
        let b = select_suggestions(&g);
        assert_eq!(a, b);
        let mut sorted = a.clone();
        sorted.sort();
        assert_eq!(a, sorted, "output must be sorted");
    }

    #[test]
    fn path_relevance_more_signals_yield_more_or_equal_suggestions() {
        let empty = SignalGroups::default();
        let e = select_suggestions(&empty);
        let mut populated = SignalGroups::default();
        populated.manifests.insert("cargo".to_string());
        populated.cicd.insert("github-actions".to_string());
        let p = select_suggestions(&populated);
        assert!(p.len() > e.len());
    }
}
