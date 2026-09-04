//! Typed command-request model — the result of parsing argv.
//!
//! Every caller (human or external agent) constructs one of these from the
//! exact approved grammar and routes it through the single [`dispatch`](crate::dispatch::dispatch)
//! boundary. The library never sees raw argv.

use std::collections::BTreeMap;

/// A typed CLI command request.
///
/// Mirrors the exact approved grammar (six SQLite-backed verbs only):
/// - `ingest <TEXT> [<METADATA-JSON>]`
/// - `query <TEXT> [<FILTER-JSON>]`
/// - `list [FILTER] [LIMIT] [OFFSET]`
/// - `get <ID>`
/// - `patch <ID> [--text <TEXT>] [--metadata <JSON>]`
/// - `remove <ID>`
///
/// Removed legacy verbs (`init`, `analyze`, `insert`, `edit`) and aliases
/// (`set`, `rm`) are not representable here; the parser rejects them as
/// usage errors before a `Command` is ever built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `ingest <TEXT> [<METADATA-JSON>]` — SQLite write path.
    Ingest {
        /// Text to ingest.
        text: String,
        /// Raw metadata JSON, if supplied.
        metadata_raw: Option<String>,
    },
    /// `query <TEXT> [<FILTER-JSON>]` — read-only retrieval.
    Query {
        /// Owned query text, preserved exactly.
        text: String,
        /// Canonical owned filter data with byte-ordered keys (`{}` when absent).
        filter: BTreeMap<String, String>,
    },
    /// `list [FILTER] [LIMIT] [OFFSET]` — read-only SQLite fetch with flat
    /// filter JSON, limit default 50 (clamped to max 500), offset default 0.
    List {
        /// Canonical owned filter data with byte-ordered keys (`{}` when absent).
        filter: BTreeMap<String, String>,
        /// Page size after clamping to max 500.
        limit: usize,
        /// Row offset into the ascending-id filtered set.
        offset: usize,
    },
    /// `get <ID>` — direct SQLite point fetch by positive integer record id.
    Get {
        /// Target record id (validated to a positive integer at execution).
        id: String,
    },
    /// `patch <ID> [--text <TEXT>] [--metadata <JSON>]` — semantic search record update.
    Patch {
        /// Target record id (validated to a positive integer at execution).
        id: String,
        /// Replacement text when `--text` was supplied.
        text: Option<String>,
        /// Raw replacement metadata JSON when `--metadata` was supplied.
        metadata_raw: Option<String>,
    },
    /// `remove <ID>` — semantic search hard delete by positive integer record id.
    Remove {
        /// Target record id (validated to a positive integer at execution).
        id: String,
    },
}

impl Command {
    /// Human-readable command name, used in diagnostics.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Ingest { .. } => "ingest",
            Command::Query { .. } => "query",
            Command::List { .. } => "list",
            Command::Get { .. } => "get",
            Command::Patch { .. } => "patch",
            Command::Remove { .. } => "remove",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_carries_id() {
        assert_eq!(
            Command::Get {
                id: "3".to_string(),
            },
            Command::Get {
                id: "3".to_string(),
            }
        );
    }

    #[test]
    fn six_verbs_have_distinct_names() {
        let names = [
            Command::Ingest {
                text: "x".to_string(),
                metadata_raw: None,
            }
            .name(),
            Command::Query {
                text: "x".to_string(),
                filter: BTreeMap::new(),
            }
            .name(),
            Command::List {
                filter: BTreeMap::new(),
                limit: 50,
                offset: 0,
            }
            .name(),
            Command::Get {
                id: "1".to_string(),
            }
            .name(),
            Command::Patch {
                id: "1".to_string(),
                text: None,
                metadata_raw: Some("{}".to_string()),
            }
            .name(),
            Command::Remove {
                id: "1".to_string(),
            }
            .name(),
        ];
        assert_eq!(names, ["ingest", "query", "list", "get", "patch", "remove"]);
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
    fn list_name_is_list() {
        assert_eq!(
            Command::List {
                filter: BTreeMap::new(),
                limit: 50,
                offset: 0,
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
