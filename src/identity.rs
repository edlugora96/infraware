//! Centralized definition-ID validation.
//!
//! This is the *only* place ID validation lives (design D3). No other module
//! may duplicate this rule; they must call [`validate_id`]. The rule is
//! deliberately conservative: a valid id is non-empty and contains only ASCII
//! letters, digits, `.`, `_`, and `-`. Anything else — whitespace, quotes,
//! newlines, or marker syntax — is rejected so ids can never break the exact
//! `<!-- infraware:definition id="..." -->` envelope.

use crate::error::LibraryError;

/// Characters (besides ASCII alphanumerics) permitted inside a definition id.
const ID_EXTRA: &[char] = &['.', '_', '-'];

/// Validate a definition id against the single centralized rule.
///
/// Returns `Ok(())` for a non-empty id composed solely of ASCII alphanumeric
/// characters plus `.`, `_`, and `-`. Returns
/// [`LibraryError::MalformedDocument`] (the canonical-document error category)
/// for any rejected id: empty, or containing whitespace, quotes, newlines, or
/// marker syntax.
pub fn validate_id(id: &str) -> Result<(), LibraryError> {
    if id.is_empty() {
        return Err(LibraryError::MalformedDocument {
            message: "definition id must not be empty".to_string(),
        });
    }
    for ch in id.chars() {
        if ch.is_ascii_alphanumeric() || ID_EXTRA.contains(&ch) {
            continue;
        }
        // Reject everything else, including whitespace, quotes, newlines, and
        // any marker-syntax characters.
        return Err(LibraryError::MalformedDocument {
            message: format!("invalid character {ch:?} in definition id {id:?}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_ids() {
        for id in ["abc", "a.b_c-1", "A1", "x-y.z_0"] {
            assert!(validate_id(id).is_ok(), "expected {id:?} to be valid");
        }
    }

    #[test]
    fn rejects_empty_id() {
        assert!(validate_id("").is_err());
    }

    #[test]
    fn rejects_whitespace() {
        assert!(validate_id("a b").is_err());
    }

    #[test]
    fn rejects_quote() {
        assert!(validate_id("a\"b").is_err());
    }

    #[test]
    fn rejects_newline() {
        assert!(validate_id("a\nb").is_err());
    }

    #[test]
    fn rejects_marker_syntax() {
        assert!(validate_id("a<!--b").is_err());
    }
}
