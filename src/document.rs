//! Canonical document model (design D1).
//!
//! The canonical `.infraware/ARCHITECTURE.md` is modeled as an ordered sequence
//! of spans. An [`Span::Unrelated`] span holds non-definition Markdown preserved
//! byte-for-byte. A [`Span::Definition`] span holds a validated
//! [`DefinitionBlock`]. Mutations replace only the targeted definition block;
//! every unrelated span is left untouched (spec: canonical document model
//! preserves unrelated Markdown).

use crate::error::LibraryError;
use crate::identity::validate_id;

/// A single definition: a validated id and its verbatim body.
///
/// The id is the *sole* source of definition identity (design D1); headings and
/// surrounding Markdown never contribute to identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionBlock {
    /// Validated definition id.
    pub id: String,
    /// Verbatim body between the exact marker pair.
    pub body: String,
}

impl DefinitionBlock {
    /// Construct a definition, validating the id through the centralized rule.
    pub fn new(id: &str, body: &str) -> Result<DefinitionBlock, LibraryError> {
        validate_id(id)?;
        Ok(DefinitionBlock {
            id: id.to_string(),
            body: body.to_string(),
        })
    }
}

/// An ordered piece of the canonical document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Span {
    /// Non-definition Markdown, preserved exactly (byte-for-byte).
    Unrelated(String),
    /// A definition block with a validated id and verbatim body.
    Definition(DefinitionBlock),
}

/// The parsed canonical document.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Document {
    /// Ordered spans: unrelated Markdown interleaved with definition blocks.
    pub spans: Vec<Span>,
}

impl Document {
    /// Number of definition blocks (unrelated spans are not counted).
    pub fn definition_count(&self) -> usize {
        self.spans
            .iter()
            .filter(|s| matches!(s, Span::Definition(_)))
            .count()
    }

    /// All definition blocks in document order.
    pub fn definitions(&self) -> impl Iterator<Item = &DefinitionBlock> {
        self.spans.iter().filter_map(|s| match s {
            Span::Definition(d) => Some(d),
            Span::Unrelated(_) => None,
        })
    }

    /// Look up a definition by id (identity is the validated id only).
    pub fn get(&self, id: &str) -> Option<&DefinitionBlock> {
        self.definitions().find(|d| d.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_accepts_valid_id() {
        let d = DefinitionBlock::new("a.b_c-1", "body").expect("valid id");
        assert_eq!(d.id, "a.b_c-1");
        assert_eq!(d.body, "body");
    }

    #[test]
    fn new_rejects_unsafe_id() {
        assert!(DefinitionBlock::new("a b", "body").is_err());
    }

    #[test]
    fn get_finds_by_id_only() {
        let doc = Document {
            spans: vec![
                Span::Unrelated("# Heading\n".to_string()),
                Span::Definition(DefinitionBlock::new("x", "v").expect("valid")),
            ],
        };
        assert_eq!(doc.definition_count(), 1);
        assert_eq!(doc.get("x").unwrap().body, "v");
        assert!(doc.get("missing").is_none());
    }
}
