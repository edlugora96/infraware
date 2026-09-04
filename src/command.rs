//! Typed command-request model — the result of parsing argv.
//!
//! Every caller (human or external agent) constructs one of these from the
//! exact approved grammar and routes it through the single [`dispatch`](crate::dispatch::dispatch)
//! boundary. The library never sees raw argv.

use std::collections::BTreeMap;

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
/// - `list [--format markdown|json|yaml]`
///
/// `analyze --suggest` is accepted here; the suggestion body is deferred to a
/// later plan and does not change this shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `init` — initialize canonical storage (behavior deferred to Plan 2).
    Init,
    /// `analyze [--format ...] [--suggest] [--details]` — read-only analysis.
    Analyze {
        /// Effective output format (defaults to [`OutputFormat::Markdown`]).
        format: OutputFormat,
        /// Whether `--suggest` was supplied (body deferred to Plan 4).
        suggest: bool,
        /// Whether `--details` was supplied (opt-in file-path evidence).
        details: bool,
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
    /// `list [--format markdown|json|yaml]` — list definition IDs (read-only).
    List {
        /// Effective output format (defaults to [`OutputFormat::Markdown`]).
        format: OutputFormat,
    },
    /// `ingest <TEXT> [<METADATA-JSON>]` — vector write path (behavior deferred).
    Ingest {
        /// Text to ingest.
        text: String,
        /// Raw metadata JSON, if supplied.
        metadata_raw: Option<String>,
    },
    /// `query <TEXT> [<FILTER-JSON>]` — read-only retrieval (behavior deferred
    /// to later tasks; parser acceptance and dispatch execution not yet wired).
    Query {
        /// Owned query text, preserved exactly.
        text: String,
        /// Canonical owned filter data with byte-ordered keys (`{}` when absent).
        filter: BTreeMap<String, String>,
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
            Command::List { .. } => "list",
            Command::Ingest { .. } => "ingest",
            Command::Query { .. } => "query",
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
                details: false,
            },
            Command::Analyze {
                format: OutputFormat::Json,
                suggest: false,
                details: false,
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

    #[test]
    fn list_default_format_is_markdown() {
        assert_eq!(
            Command::List {
                format: OutputFormat::Markdown
            },
            Command::List {
                format: OutputFormat::default()
            }
        );
    }

    #[test]
    fn list_carries_explicit_format() {
        assert_eq!(
            Command::List {
                format: OutputFormat::Json
            },
            Command::List {
                format: OutputFormat::Json
            }
        );
        assert_eq!(
            Command::List {
                format: OutputFormat::Yaml
            },
            Command::List {
                format: OutputFormat::Yaml
            }
        );
    }

    #[test]
    fn list_name_is_list() {
        assert_eq!(
            Command::List {
                format: OutputFormat::Markdown
            }
            .name(),
            "list"
        );
    }

    #[test]
    fn ingest_name_is_ingest() {
        assert_eq!(
            Command::Ingest {
                text: "x".to_string(),
                metadata_raw: None,
            }
            .name(),
            "ingest"
        );
    }
}
