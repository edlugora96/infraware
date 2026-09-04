//! Shared definition model (design D2, requirement: shared definition model
//! across projections).
//!
//! Every definition yielded by a `get` projection is derived from this single
//! in-memory model, so Markdown, JSON, and YAML projections can never diverge.
//! The id is validated through Plan 2's centralized rule (design D3); this
//! module never re-implements ID validation.

use crate::error::LibraryError;
use crate::identity::validate_id;
use serde::{Deserialize, Serialize};

/// A single definition: a validated id and its verbatim body.
///
/// This is the one model every projection serializes from. Field naming is
/// stable (`id`, `body`) and is part of the JSON/YAML contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Definition {
    /// Validated definition id (the sole identity).
    pub id: String,
    /// Verbatim body of the definition.
    pub body: String,
}

impl Definition {
    /// Construct a definition, validating the id through the centralized rule.
    ///
    /// Returns [`LibraryError::MalformedDocument`] for a rejected id so callers
    /// map it to the same typed error category Plan 2 uses (design D3, no local
    /// duplicate validation logic).
    pub fn new(id: &str, body: &str) -> Result<Definition, LibraryError> {
        validate_id(id)?;
        Ok(Definition {
            id: id.to_string(),
            body: body.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_id_succeeds() {
        let d = Definition::new("a.b_c-1", "body").expect("valid id");
        assert_eq!(d.id, "a.b_c-1");
        assert_eq!(d.body, "body");
    }

    #[test]
    fn invalid_id_is_rejected_via_shared_rule() {
        // Whitespace is rejected by the centralized rule, not local logic.
        assert!(Definition::new("a b", "body").is_err());
    }
}
