//! Typed command-request model — the result of parsing argv.
//!
//! Every caller (human or external agent) constructs one of these from the
//! exact approved grammar and routes it through the single [`dispatch`](crate::dispatch::dispatch)
//! boundary. The library never sees raw argv.

/// A typed CLI command request.
///
/// Mirrors the exact approved grammar (six SQLite-backed verbs only):
/// - `ingest <FACT_TEXT> <QUERY_TEXT>`
/// - `query <TEXT>`
/// - `list [LIMIT] [OFFSET]`
/// - `get <ID>`
/// - `patch <ID> --text <TEXT>`
/// - `remove <ID>`
///
/// Removed legacy verbs (`init`, `analyze`, `insert`, `edit`) and aliases
/// (`set`, `rm`) are not representable here; the parser rejects them as
/// usage errors before a `Command` is ever built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `ingest <FACT_TEXT> <QUERY_TEXT>` — SQLite write path.
    Ingest {
        /// Canonical fact text, preserved exactly.
        fact_text: String,
        /// Canonical paired query text, preserved exactly.
        query_text: String,
    },
    /// `query <TEXT>` — read-only retrieval.
    Query {
        /// Owned query text, preserved exactly.
        text: String,
    },
    /// `list [LIMIT] [OFFSET]` — read-only SQLite fetch.
    List {
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
    /// `patch <ID> --text <TEXT>` — record text update.
    Patch {
        /// Target record id (validated to a positive integer at execution).
        id: String,
        /// Replacement text when `--text` was supplied.
        text: Option<String>,
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
