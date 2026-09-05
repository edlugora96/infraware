//! Typed library error model.
//!
//! Exposes at least the externally observable categories required by the
//! foundation: invalid input (usage/parse), missing definition, uninitialized
//! canonical storage, malformed canonical document, and I/O failure. Later
//! plans may refine variants without changing the CLI contract.

use std::error::Error;
use std::fmt;

/// Typed library error.
///
/// Each variant carries enough context to produce a clear stderr diagnostic.
/// `is_usage()` distinguishes parse/usage failures (→ non-zero usage status)
/// from operational failures (→ generic non-zero status).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryError {
    /// Invalid input: a usage/parse failure at the CLI boundary.
    InvalidInput {
        /// Human-readable explanation of the usage violation.
        message: String,
    },
    /// A referenced definition does not exist (for `get`/`edit`/`remove`/`insert`).
    MissingDefinition {
        /// The requested definition id.
        id: String,
    },
    /// Canonical storage has not been initialized.
    UninitializedStorage,
    /// A canonical document is malformed and cannot be read.
    MalformedDocument {
        /// Human-readable description of the malformation.
        message: String,
    },
    /// Repository settings are malformed or cannot be read.
    Configuration {
        /// Human-readable configuration diagnostic.
        message: String,
    },
    /// An I/O operation failed.
    Io {
        /// Human-readable description of the I/O failure.
        message: String,
    },
}

impl LibraryError {
    /// Whether this error is a usage/parse error that maps to the parser's
    /// non-zero usage status.
    pub fn is_usage(&self) -> bool {
        matches!(self, LibraryError::InvalidInput { .. })
    }
}

impl fmt::Display for LibraryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LibraryError::InvalidInput { message } => {
                write!(f, "invalid input: {message}")
            }
            LibraryError::MissingDefinition { id } => {
                write!(f, "missing definition: {id}")
            }
            LibraryError::UninitializedStorage => {
                write!(f, "canonical storage is not initialized")
            }
            LibraryError::MalformedDocument { message } => {
                write!(f, "malformed canonical document: {message}")
            }
            LibraryError::Configuration { message } => write!(f, "configuration error: {message}"),
            LibraryError::Io { message } => write!(f, "i/o error: {message}"),
        }
    }
}

impl Error for LibraryError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_input_is_usage() {
        assert!(
            LibraryError::InvalidInput {
                message: "bad".to_string()
            }
            .is_usage()
        );
    }

    #[test]
    fn non_usage_categories_are_not_usage() {
        assert!(
            !LibraryError::MissingDefinition {
                id: "x".to_string()
            }
            .is_usage()
        );
        assert!(!LibraryError::UninitializedStorage.is_usage());
        assert!(
            !LibraryError::MalformedDocument {
                message: "m".to_string()
            }
            .is_usage()
        );
        assert!(
            !LibraryError::Io {
                message: "m".to_string()
            }
            .is_usage()
        );
    }

    #[test]
    fn display_produces_clear_diagnostics() {
        assert_eq!(
            LibraryError::MissingDefinition {
                id: "abc".to_string()
            }
            .to_string(),
            "missing definition: abc"
        );
        assert_eq!(
            LibraryError::UninitializedStorage.to_string(),
            "canonical storage is not initialized"
        );
        assert_eq!(
            LibraryError::InvalidInput {
                message: "unknown command".to_string()
            }
            .to_string(),
            "invalid input: unknown command"
        );
    }
}
