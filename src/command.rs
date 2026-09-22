//! Typed command-request model — the result of parsing argv.
//!
//! Every caller (human or external agent) constructs one of these from the
//! exact approved grammar and routes it through the single [`dispatch`](crate::dispatch::dispatch)
//! boundary. The library never sees raw argv.

/// A closed selector for a single entity table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entity {
    /// Rule records.
    Rules,
    /// Task records.
    Tasks,
}

/// Shared output controls for every verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvocationOptions {
    pub format: crate::format::OutputFormat,
    pub detailed: bool,
}

/// A typed CLI command request.
///
/// Mirrors the exact approved grammar (nine SQLite-backed verbs):
/// - `init`
/// - `ingest <TASK_TEXT> <RULE_TEXT>
/// - `query <TASK_TEXT>`
/// - `list [LIMIT] [OFFSET]`
/// - `get <ID>`
/// - `patch <ID> --text <TEXT>`
/// - `remove <ID> --entity rules|tasks`
/// - `status`
/// - `revise <ID> --entity rules|tasks`
///
/// Removed legacy verbs (`analyze`, `insert`, `edit`) and aliases (`set`, `rm`)
/// are not representable here; the parser rejects them as usage errors before a
/// `Command` is ever built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `init` - create or verify canonical storage.
    Init,
    /// `status` - read-only staleness report.
    Status,
    /// `ingest <TASK_TEXT> <RULE_TEXT>` — SQLite write path.
    Ingest {
        /// Canonical task text, preserved exactly.
        task_text: String,
        /// Canonical paired rule text, preserved exactly.
        rule_text: String,
    },
    /// `query <TASK_TEXT>` — read-only retrieval.
    Query {
        /// Owned task text, preserved exactly.
        text: String,
    },
    /// `list [LIMIT] [OFFSET]` — read-only SQLite fetch.
    List {
        /// Page size after clamping to max 500.
        limit: usize,
        /// Row offset into the ascending-id rule set.
        offset: usize,
    },
    /// `get <ID>` — direct SQLite point fetch by positive integer record id.
    Get {
        /// Target record id (validated to a positive integer at execution).
        id: String,
        /// Entity table to read.
        entity: Entity,
    },
    /// `patch <ID> --text <TEXT>` — record text update.
    Patch {
        /// Target record id (validated to a positive integer at execution).
        id: String,
        /// Replacement text when `--text` was supplied.
        text: Option<String>,
        /// Entity table to patch.
        entity: Entity,
    },
    /// `remove <ID> --entity rules|tasks` — semantic search hard delete by positive integer record id.
    Remove {
        /// Target record id (validated to a positive integer at execution).
        id: String,
        /// Entity table to remove from.
        entity: Entity,
    },
    /// `revise <ID> --entity rules|tasks` — timestamp-only touch.
    Revise {
        /// Target record id.
        id: String,
        /// Entity table to revise.
        entity: Entity,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub command: Command,
    pub options: InvocationOptions,
}

impl Command {
    /// Human-readable command name, used in diagnostics.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Init => "init",
            Command::Status => "status",
            Command::Ingest { .. } => "ingest",
            Command::Query { .. } => "query",
            Command::List { .. } => "list",
            Command::Get { .. } => "get",
            Command::Patch { .. } => "patch",
            Command::Remove { .. } => "remove",
            Command::Revise { .. } => "revise",
        }
    }
}
