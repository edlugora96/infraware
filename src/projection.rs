//! Single serializer boundary for definition projections (design D6,
//! requirement: shared definition model across projections, JSON-primary and
//! YAML output-only).
//!
//! Every format is rendered from the shared [`Definition`](crate::definition_model::Definition)
//! model. Markdown is the canonical/readable form, JSON is the primary
//! machine-readable projection, and YAML is output-only. None of these ever
//! write the canonical Markdown document.

use crate::definition_model::Definition;
use crate::error::LibraryError;
use crate::format::OutputFormat;

/// Render a definition to the requested [`OutputFormat`].
///
/// - [`OutputFormat::Markdown`] — a readable Markdown block wrapping the body in
///   the exact marker pair (the canonical envelope, read-only here).
/// - [`OutputFormat::Json`] — primary machine-readable projection.
/// - [`OutputFormat::Yaml`] — output-only YAML projection.
///
/// Parsing/rejecting invalid formats is the parser's job (`--format xml` is a
/// usage error before this is ever called), so this function trusts its input.
pub fn serialize_definition(
    definition: &Definition,
    format: OutputFormat,
) -> Result<String, LibraryError> {
    match format {
        OutputFormat::Markdown => Ok(render_markdown(definition)),
        OutputFormat::Json => {
            serde_json::to_string_pretty(definition).map_err(|e| LibraryError::MalformedDocument {
                message: format!("failed to serialize JSON projection: {e}"),
            })
        }
        OutputFormat::Yaml => {
            serde_yaml::to_string(definition).map_err(|e| LibraryError::MalformedDocument {
                message: format!("failed to serialize YAML projection: {e}"),
            })
        }
    }
}

/// Render the canonical/readable Markdown form of a definition.
///
/// Uses the exact marker pair from Plan 2 so a Markdown projection reads like a
/// real document block. This is output-only and never persisted as canonical
/// storage (that path is `persistence::write_atomic`).
fn render_markdown(definition: &Definition) -> String {
    let mut out = String::new();
    out.push_str(crate::document_parser::OPEN_PREFIX);
    out.push_str(&definition.id);
    out.push_str(crate::document_parser::OPEN_SUFFIX);
    out.push_str(&definition.body);
    out.push_str(crate::document_parser::CLOSE);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_parser::{CLOSE, OPEN_PREFIX, OPEN_SUFFIX};

    fn fixture() -> Definition {
        Definition::new("abc", "hello world").expect("valid id")
    }

    #[test]
    fn markdown_projection_uses_exact_markers() {
        let out = serialize_definition(&fixture(), OutputFormat::Markdown).unwrap();
        assert_eq!(
            out,
            format!("{OPEN_PREFIX}abc{OPEN_SUFFIX}hello world{CLOSE}")
        );
    }

    #[test]
    fn json_projection_is_object_with_id_and_body() {
        let out = serialize_definition(&fixture(), OutputFormat::Json).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["id"], "abc");
        assert_eq!(parsed["body"], "hello world");
    }

    #[test]
    fn yaml_projection_is_object_with_id_and_body() {
        let out = serialize_definition(&fixture(), OutputFormat::Yaml).unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&out).unwrap();
        assert_eq!(parsed["id"].as_str(), Some("abc"));
        assert_eq!(parsed["body"].as_str(), Some("hello world"));
    }

    #[test]
    fn all_formats_share_identical_id_and_body() {
        let d = fixture();
        for fmt in [
            OutputFormat::Markdown,
            OutputFormat::Json,
            OutputFormat::Yaml,
        ] {
            let out = serialize_definition(&d, fmt).unwrap();
            // Every projection must reference the same id/body.
            assert!(out.contains("abc"), "format {fmt} missing id");
            assert!(out.contains("hello world"), "format {fmt} missing body");
        }
    }

    #[test]
    fn json_is_deterministic() {
        let a = serialize_definition(&fixture(), OutputFormat::Json).unwrap();
        let b = serialize_definition(&fixture(), OutputFormat::Json).unwrap();
        assert_eq!(a, b, "JSON projection must be byte-stable");
    }

    #[test]
    fn yaml_is_deterministic() {
        let a = serialize_definition(&fixture(), OutputFormat::Yaml).unwrap();
        let b = serialize_definition(&fixture(), OutputFormat::Yaml).unwrap();
        assert_eq!(a, b, "YAML projection must be byte-stable");
    }

    #[test]
    fn round_trip_definition_via_json_yields_same_model() {
        let d = fixture();
        let json = serialize_definition(&d, OutputFormat::Json).unwrap();
        let back: Definition = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
    }
}
