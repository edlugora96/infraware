//! Typed command-request model — the result of parsing argv.
//!
//! Every caller (human or external agent) constructs one of these from the
//! exact approved grammar and routes it through the single [`dispatch`](crate::dispatch::dispatch)
//! boundary. The library never sees raw argv.

use crate::format::OutputFormat;

/// A typed CLI command request.
///
/// Mirrors the exact approved grammar:
/// - `init`
/// - `analyze [--format markdown|json|yaml] [--suggest]`
/// - `get <ID> [--format markdown|json|yaml]`
/// - `insert <ID> --value <TEXT>`
/// - `edit <ID> --value <TEXT>`
/// - `remove <ID>`
///
/// `analyze --suggest` is accepted here; the suggestion body is deferred to a
/// later plan and does not change this shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `init` — initialize canonical storage (behavior deferred to Plan 2).
    Init,
    /// `analyze [--format ...] [--suggest]` — read-only analysis.
    Analyze {
        /// Effective output format (defaults to [`OutputFormat::Markdown`]).
        format: OutputFormat,
        /// Whether `--suggest` was supplied (body deferred to Plan 4).
        suggest: bool,
    },
    /// `get <ID> [--format ...]` — fetch a definition by id.
    Get {
        /// Target definition id.
        id: String,
        /// Effective output format (defaults to [`OutputFormat::Markdown`]).
        format: OutputFormat,
    },
    /// `insert <ID> --value <TEXT>` — create/replace a definition.
    Insert {
        /// Target definition id.
        id: String,
        /// Definition text.
        value: String,
    },
    /// `edit <ID> --value <TEXT>` — edit an existing definition.
    Edit {
        /// Target definition id.
        id: String,
        /// New definition text.
        value: String,
    },
    /// `remove <ID>` — remove a definition by id.
    Remove {
        /// Target definition id.
        id: String,
    },
}

impl Command {
    /// Human-readable command name, used in diagnostics.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Init => "init",
            Command::Analyze { .. } => "analyze",
            Command::Get { .. } => "get",
            Command::Insert { .. } => "insert",
            Command::Edit { .. } => "edit",
            Command::Remove { .. } => "remove",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::OutputFormat;

    #[test]
    fn get_carries_id_and_default_format() {
        assert_eq!(
            Command::Get {
                id: "abc".to_string(),
                format: OutputFormat::Markdown,
            },
            Command::Get {
                id: "abc".to_string(),
                format: OutputFormat::default(),
            }
        );
    }

    #[test]
    fn analyze_explicit_format_is_honored() {
        assert_eq!(
            Command::Analyze {
                format: OutputFormat::Json,
                suggest: false,
            },
            Command::Analyze {
                format: OutputFormat::Json,
                suggest: false,
            }
        );
    }

    #[test]
    fn remove_carry_only_id() {
        assert_eq!(
            Command::Remove {
                id: "abc".to_string(),
            },
            Command::Remove {
                id: "abc".to_string(),
            }
        );
    }
}
