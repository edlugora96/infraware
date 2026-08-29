//! Canonical-document parser and serializer (design D2).
//!
//! The parser recognizes *only* the reserved `infraware:definition` marker
//! namespace and yields the ordered [`Document`] model, returning typed errors
//! (never panics) for malformed or unbalanced markers. The serializer re-emits
//! unrelated spans verbatim and re-wraps each definition block with the exact
//! marker pair.

use crate::document::{DefinitionBlock, Document, Span};
use crate::error::LibraryError;
use crate::identity::validate_id;

/// Exact opening marker prefix: `<!-- infraware:definition id="`.
pub const OPEN_PREFIX: &str = "<!-- infraware:definition id=\"";
/// Exact opening marker suffix: `" -->`.
pub const OPEN_SUFFIX: &str = "\" -->";
/// Exact closing marker: `<!-- /infraware:definition -->`.
pub const CLOSE: &str = "<!-- /infraware:definition -->";

/// Reserved-namespace prefix used to detect structurally-invalid markers
/// (e.g., a typo'd or malformed `infraware:definition` tag).
const NS_PREFIX: &str = "<!-- infraware:definition";

/// Parse canonical Markdown into a [`Document`].
///
/// Recognizes only the exact opening/closing marker pair. Returns a typed
/// [`LibraryError::MalformedDocument`] (never panics) for: a reserved-namespace
/// marker that is structurally invalid, an unmatched opening or closing marker,
/// or a nested/overlapping definition. A marker-free document is a valid empty
/// document (zero definition blocks; unrelated Markdown preserved).
pub fn parse_document(text: &str) -> Result<Document, LibraryError> {
    let mut spans: Vec<Span> = Vec::new();
    let mut pos = 0usize;

    loop {
        // Detect any reserved-namespace marker so we can reject forms that are
        // structurally invalid (e.g., a typo'd `infraware:definition` tag that
        // is not the exact open form). Foreign namespaces are left as unrelated
        // text and never rejected here.
        if let Some(ns_rel) = text[pos..].find(NS_PREFIX) {
            let ns = pos + ns_rel;
            if !text[ns..].starts_with(OPEN_PREFIX) {
                return Err(LibraryError::MalformedDocument {
                    message: "malformed infraware:definition marker".to_string(),
                });
            }
        }

        match (text[pos..].find(OPEN_PREFIX), text[pos..].find(CLOSE)) {
            // No markers remain: the rest is unrelated Markdown.
            (None, None) => {
                let tail = &text[pos..];
                if !tail.is_empty() {
                    spans.push(Span::Unrelated(tail.to_string()));
                }
                break;
            }
            // A closing marker with no opening before it is unbalanced.
            (None, Some(_)) => {
                return Err(LibraryError::MalformedDocument {
                    message: "unbalanced definition marker: closing without opening".to_string(),
                });
            }
            (Some(open_rel), close_rel) => {
                let open_start = pos + open_rel;
                let before = &text[pos..open_start];
                if !before.is_empty() {
                    spans.push(Span::Unrelated(before.to_string()));
                }

                let after_prefix = open_start + OPEN_PREFIX.len();
                let suffix_rel = text[after_prefix..].find(OPEN_SUFFIX).ok_or_else(|| {
                    LibraryError::MalformedDocument {
                        message: "malformed definition marker: unterminated opening tag"
                            .to_string(),
                    }
                })?;
                let id = &text[after_prefix..after_prefix + suffix_rel];
                validate_id(id)?;

                let open_end = after_prefix + suffix_rel + OPEN_SUFFIX.len();

                // A closing marker before this opening is unbalanced.
                if let Some(c) = close_rel {
                    if pos + c < open_start {
                        return Err(LibraryError::MalformedDocument {
                            message: "unbalanced definition marker: closing before opening"
                                .to_string(),
                        });
                    }
                }

                let close_rel2 = text[open_end..].find(CLOSE).ok_or_else(|| {
                    LibraryError::MalformedDocument {
                        message: "unbalanced definition marker: missing closing tag".to_string(),
                    }
                })?;
                let body = &text[open_end..open_end + close_rel2];
                // A nested opening inside the body is structurally invalid.
                if body.contains(OPEN_PREFIX) {
                    return Err(LibraryError::MalformedDocument {
                        message: "malformed definition marker: nested definition".to_string(),
                    });
                }

                spans.push(Span::Definition(DefinitionBlock {
                    id: id.to_string(),
                    body: body.to_string(),
                }));
                pos = open_end + close_rel2 + CLOSE.len();
            }
        }
    }

    Ok(Document { spans })
}

/// Serialize a [`Document`] back to canonical Markdown.
///
/// Unrelated spans are emitted verbatim; each definition block is re-wrapped
/// with the exact marker pair. A document parsed and serialized without
/// modification is byte-identical to the input.
pub fn serialize_document(doc: &Document) -> String {
    let mut out = String::new();
    for span in &doc.spans {
        match span {
            Span::Unrelated(text) => out.push_str(text),
            Span::Definition(def) => {
                out.push_str(OPEN_PREFIX);
                out.push_str(&def.id);
                out.push_str(OPEN_SUFFIX);
                out.push_str(&def.body);
                out.push_str(CLOSE);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEF: &str = "<!-- infraware:definition id=\"abc\" -->body<!-- /infraware:definition -->";

    #[test]
    fn parses_valid_definition() {
        let doc = parse_document(DEF).expect("valid marker pair parses");
        assert_eq!(doc.definition_count(), 1);
        let d = doc.get("abc").expect("definition present");
        assert_eq!(d.body, "body");
    }

    #[test]
    fn round_trips_marker_free_document() {
        let src = "# Heading\n\nprose with `code` and <!-- an html comment --> and trailing whitespace  \n\n";
        let doc = parse_document(src).expect("parse");
        assert_eq!(doc.definition_count(), 0);
        assert_eq!(doc.spans.len(), 1);
        assert_eq!(serialize_document(&doc), src);
    }

    #[test]
    fn round_trips_with_unrelated_content() {
        let src = "# Title\n\nSome prose.\n\n<!-- infraware:definition id=\"x\" -->\nhello\n<!-- /infraware:definition -->\n\nTrailing.\n";
        let doc = parse_document(src).expect("parse");
        assert_eq!(serialize_document(&doc), src);
    }

    #[test]
    fn marker_free_is_empty_document() {
        let src = "just prose, no markers";
        let doc = parse_document(src).expect("parse");
        assert_eq!(doc.definition_count(), 0);
    }

    #[test]
    fn unmatched_opening_is_error() {
        let src = "<!-- infraware:definition id=\"x\" -->body";
        assert!(parse_document(src).is_err());
    }

    #[test]
    fn unmatched_closing_is_error() {
        let src = "prose<!-- /infraware:definition -->";
        assert!(parse_document(src).is_err());
    }

    #[test]
    fn wrong_namespace_is_error() {
        // Reserved namespace present but structurally invalid (typo'd tag).
        let src = "<!-- infraware:definitionn id=\"x\" -->body<!-- /infraware:definition -->";
        assert!(parse_document(src).is_err());
    }

    #[test]
    fn unsafe_id_is_error() {
        let src = "<!-- infraware:definition id=\"a b\" -->body<!-- /infraware:definition -->";
        assert!(parse_document(src).is_err());
    }

    #[test]
    fn nested_definition_is_error() {
        let inner = "<!-- infraware:definition id=\"inner\" -->x<!-- /infraware:definition -->";
        let src = format!(
            "<!-- infraware:definition id=\"outer\" -->{inner}<!-- /infraware:definition -->"
        );
        assert!(parse_document(&src).is_err());
    }
}
