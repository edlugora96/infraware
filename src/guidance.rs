//! Static, read-only guidance catalog (design D5).
//!
//! One immutable catalog of general guidance. `init` renders it as comments
//! only; later read-only selectors (Plan 4) consume it as data. The catalog is
//! never written as definitions or mandatory schema and has no canonical-write
//! path.

/// General guidance lines rendered by `init` as Markdown comments.
///
/// Static and project-agnostic. The precise wording is an implementation detail
/// drawn from the design's open question on helpful-comment text; it is not a
/// definition or mandatory schema.
pub const CATALOG: &[&str] = &[
    "infraware canonical architecture document.",
    "This file is the single source of truth for architecture definitions.",
    "Edit it only through the CLI; do not hand-edit definition markers.",
    "Optional lifecycle areas you may document: context, constraints, decisions, risks.",
    "Each definition is identified solely by its id between the exact marker pair.",
];

/// Render the catalog as Markdown HTML comments.
///
/// Returns one `<!-- ... -->` comment per catalog line. This is the only form
/// the catalog takes when written by `init`; it never becomes a definition or
/// mandatory schema.
pub fn render_comments() -> String {
    let mut out = String::new();
    for line in CATALOG {
        out.push_str("<!-- ");
        out.push_str(line);
        out.push_str(" -->\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_non_empty() {
        assert!(!CATALOG.is_empty());
    }

    #[test]
    fn render_wraps_each_line_as_comment() {
        let rendered = render_comments();
        for line in CATALOG {
            assert!(
                rendered.contains(&format!("<!-- {line} -->")),
                "missing {line}"
            );
        }
        // Every catalog line is wrapped, so the count of opening comments equals
        // the catalog length.
        assert_eq!(rendered.matches("<!-- ").count(), CATALOG.len());
    }
}
