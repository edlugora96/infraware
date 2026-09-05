//! `infraware` — caller-agnostic CLI foundation for the eight SQLite-backed semantic search verbs.
//!
//! This crate establishes the single-package Rust boundary for `ingest`,
//! `query`, `list`, `get`, `patch`, `remove`, `status`, and `revise` against the SQLite vector
//! index. It intentionally contains **no** LLM/network/provider/plugin/workspace/
//! caller-specific code. Everything reachable from the binary goes through one
//! dispatch seam.
//!
//! Module layout:
//! - [`format`] — shared typed output-format value (JSON).
//! - [`command`] — typed command-request enum (the parse result).
//! - [`error`] — typed library error model.
//! - [`parser`] — stdlib-only exact-argument parser.
//! - [`dispatch`] — the single caller-independent dispatch boundary.
//! - [`run`] — the thin binary adapter (parse → dispatch → streams/status).

#![deny(unsafe_code)]

mod chrono_format;
mod command;
mod dispatch;
mod embed;
mod error;
mod format;
mod ingest;
mod model_cache;
mod parser;
mod patch;
mod query;
mod records;
mod remove;
mod render;
mod revise;
mod settings;
mod staleness;
mod status;
mod vec_shim;
mod vec_store;

pub use command::{Command, Entity, Invocation, InvocationOptions};
pub use dispatch::{DispatchOutput, dispatch};
pub use error::LibraryError;
pub use format::OutputFormat;
pub use parser::parse_args;

use std::io::{self, Write};

/// Usage exit status for argument/usage errors (unknown command, unknown
/// option, malformed shape, invalid format). Distinct from the generic
/// operational non-zero status so tooling can tell the two apart.
pub const EXIT_USAGE: u8 = 2;

/// Operational non-zero status for dispatch/operational failures (record-not-found,
/// missing or incompatible store, backend errors).
pub const EXIT_FAILURE: u8 = 1;

const HELP_TEXT: &str = "Usage: infraware <COMMAND>\n\nCommands:\n  ingest <TASK_TEXT> <RULE_TEXT>\n  query <TASK_TEXT>\n  list [LIMIT] [OFFSET]\n  get <ID> [--entity rules|tasks]\n  patch <ID> --text <TEXT> [--entity rules|tasks]\n  remove <ID>\n  status\n  revise <ID> --entity rules|tasks";

fn write_help<W: Write>(stdout: &mut W) {
    let _ = writeln!(stdout, "{HELP_TEXT}");
}

/// Thin binary adapter: parse argv → build typed [`Command`] → invoke the single
/// library dispatch boundary → write structured output to `stdout` and
/// diagnostics to `stderr` → return the process [`ExitCode`].
///
/// This function owns no business rules. It translates typed results/errors into
/// the frozen stream/status contract and never emits structured output on
/// `stderr` or diagnostics on `stdout`.
pub fn run<I, W1, W2>(args: I, stdout: &mut W1, stderr: &mut W2) -> u8
where
    I: IntoIterator<Item = String>,
    W1: Write,
    W2: Write,
{
    let argv: Vec<String> = args.into_iter().collect();

    if argv.is_empty() || (argv.len() == 1 && argv[0] == "--help") {
        write_help(stdout);
        return 0;
    }
    if argv.len() == 1 && argv[0] == "--version" {
        let _ = writeln!(stdout, "infraware {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }

    // Stage 1: parse. Usage errors short-circuit to the usage status.
    let command = match parse_args(&argv) {
        Ok(command) => command,
        Err(error) => {
            let _ = writeln!(stderr, "error: {error}");
            return EXIT_USAGE;
        }
    };

    // Stage 2: single caller-independent dispatch. Resolve the repository root
    // (current working directory) the filesystem-backed commands operate on.
    let root = match std::env::current_dir() {
        Ok(root) => root,
        Err(error) => {
            // I/O failure to resolve the working directory is a non-usage
            // operational failure: diagnostics to stderr, generic non-zero status.
            let _ = writeln!(
                stderr,
                "error: i/o error: failed to resolve current directory: {error}"
            );
            return EXIT_FAILURE;
        }
    };

    match dispatch(command, &root) {
        Ok(output) => {
            // Structured data → stdout only. Empty bodies are harmless and keep
            // the stream contract explicit.
            if !output.body.is_empty() {
                let _ = stdout.write_all(output.body.as_bytes());
                if !output.body.ends_with('\n') {
                    let _ = stdout.write_all(b"\n");
                }
            }
            0
        }
        Err(error) => {
            // Diagnostics → stderr only. No structured output is written.
            let _ = writeln!(stderr, "error: {error}");
            if error.is_usage() {
                EXIT_USAGE
            } else {
                EXIT_FAILURE
            }
        }
    }
}

/// Process entry point. Delegates to [`run`] with the real standard streams.
///
/// Kept deliberately thin: it only forwards argv and the std streams. All
/// behavior lives in the library modules above.
pub fn main_entry() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();
    std::process::ExitCode::from(run(args, &mut stdout, &mut stderr))
}
