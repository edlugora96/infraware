//! The single caller-independent dispatch boundary.

use crate::command::{Command, Invocation};
use crate::error::LibraryError;
use crate::format::OutputFormat;
use crate::ingest;
use std::path::Path;

/// Structured output produced by dispatch for a successful command.
///
/// `body` is the structured data destined for stdout; `format` records the
/// format it was rendered in so callers/streams need no further knowledge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchOutput {
    /// Output format the body was rendered in.
    pub format: OutputFormat,
    /// Structured payload for stdout (may be empty for the foundation).
    pub body: String,
}

/// Caller-independent dispatch: route a typed [`Command`] to its behavior.
///
/// This is the **only** seam through which later plans implement behavior. It
/// does not inspect caller identity, and it contains no LLM, provider, network,
/// plugin, or agent code (design D4/D5, spec: Caller-independent typed dispatch,
/// No LLM/network/provider/plugin/workspace code).
///
/// `root` is the repository root (current working directory) that filesystem-
/// backed commands operate on. It is threaded through the single boundary so
/// the callers (binary, agents) never pass it per-command; the boundary owns
/// the only canonical-storage touchpoint (design D4).
///
/// The frozen stream/status contract is preserved: usage failures map to
/// status 2 with empty stdout; operational failures (missing/incompatible
/// store, record-not-found, backend errors) map to status 1 with empty
/// stdout and one `error:` line on stderr.
///
/// Exactly the eight SQLite-backed verbs are routed, each to its thin core:
/// - `ingest`/`query` keep their existing SQLite paths.
/// - `status` reports freshness through its existing read path.
/// - `list`/`get` read through the approved read-only snapshot lane.
/// - `patch`/`remove`/`revise` mutate through the approved read-write lane.
pub fn dispatch(invocation: Invocation, root: &Path) -> Result<DispatchOutput, LibraryError> {
    let format = invocation.options.format;
    let detailed = invocation.options.detailed;
    let command = invocation.command;
    let name = command.name();
    let mut output = match command {
        Command::Status => {
            let s = crate::settings::load(root)?;
            crate::status::run(root, s)
        }
        Command::Ingest {
            task_text,
            rule_text,
        } => {
            let s = crate::settings::load(root)?;
            ingest::run(root, &task_text, &rule_text, s)
        }
        // Task 2.3: `query` execution seam. The arm only routes the owned
        // parser payload to `query::run`; behavior stays deferred to tasks
        // 3.x-6.x. The stub returns a non-usage operational error, so `run`
        // maps it to status 1 with empty stdout (never usage status 2).
        Command::Query { text } => {
            let s = crate::settings::load(root)?;
            crate::query::run(root, &text, s.query_max, s.query_auto_ingest)
        }
        // Read-only SQLite verbs through the thin records cores: direct
        // point fetch (`get`) and ascending-id snapshot scan (`list`).
        Command::List {
            limit,
            offset,
            entity,
        } => crate::records::list(root, limit, offset, entity),
        Command::Get { id, entity } => crate::records::get(root, &id, entity),
        Command::Patch { id, text, entity } => {
            // Patch validates its complete payload before settings/configuration
            // or storage I/O, so malformed syntax has usage precedence.
            crate::patch::run(root, &id, text.as_deref(), entity)
        }
        Command::Remove { id } => {
            // Hard delete by positive integer record id. ID shape,
            // storage, and record-not-found rules live in `remove::run`.
            crate::remove::run(root, &id)
        }
        Command::Revise { id, entity } => crate::revise::run(root, &id, entity),
    }?;
    output.body = crate::render::render(name, &output.body, format, detailed)?;
    output.format = format;
    Ok(output)
}
