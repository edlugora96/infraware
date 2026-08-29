//! The single caller-independent dispatch boundary.

use crate::command::Command;
use crate::error::LibraryError;
use crate::format::OutputFormat;

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

/// Caller-independent dispatch: route a typed [`Command`] to its (deferred)
/// behavior.
///
/// This is the **only** seam through which later plans implement behavior. It
/// does not inspect caller identity, and it contains no LLM, provider, network,
/// plugin, or agent code (design D4/D5, spec: Caller-independent typed dispatch,
/// No LLM/network/provider/plugin/workspace code).
///
/// Because business logic, persistence, scanning, projections, and suggestions
/// are explicitly out of scope for Plan 1, the foundation encodes only the
/// frozen stream/status contract:
/// - `init` succeeds with no structured body (storage creation deferred to Plan 2).
/// - `analyze` succeeds with an empty body: empty/incomplete analysis is
///   successful and finalized by Plan 4.
/// - `get`/`insert`/`edit`/`remove` require canonical storage that does not
///   exist yet, so they return the corresponding typed error (non-zero).
pub fn dispatch(command: Command) -> Result<DispatchOutput, LibraryError> {
    match command {
        Command::Init => Ok(DispatchOutput {
            format: OutputFormat::Markdown,
            body: String::new(),
        }),
        Command::Analyze { format, .. } => Ok(DispatchOutput {
            format,
            body: String::new(),
        }),
        // Storage-backed operations are deferred to later plans. The canonical
        // storage is not initialized in the foundation package, so these return
        // typed errors rather than fabricating data.
        Command::Get { id, .. } => Err(LibraryError::MissingDefinition { id }),
        Command::Insert { .. } => Err(LibraryError::UninitializedStorage),
        Command::Edit { .. } => Err(LibraryError::UninitializedStorage),
        Command::Remove { .. } => Err(LibraryError::UninitializedStorage),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;

    #[test]
    fn init_succeeds_empty() {
        let out = dispatch(Command::Init).expect("init succeeds");
        assert_eq!(out.format, OutputFormat::Markdown);
        assert!(out.body.is_empty());
    }

    #[test]
    fn analyze_empty_is_success() {
        let out = dispatch(Command::Analyze {
            format: OutputFormat::Json,
            suggest: true,
        })
        .expect("empty analyze succeeds");
        assert_eq!(out.format, OutputFormat::Json);
        assert!(out.body.is_empty());
    }

    #[test]
    fn get_is_missing_definition() {
        assert_eq!(
            dispatch(Command::Get {
                id: "abc".to_string(),
                format: OutputFormat::Markdown,
            }),
            Err(LibraryError::MissingDefinition {
                id: "abc".to_string()
            })
        );
    }

    #[test]
    fn insert_requires_storage() {
        assert_eq!(
            dispatch(Command::Insert {
                id: "abc".to_string(),
                value: "x".to_string(),
            }),
            Err(LibraryError::UninitializedStorage)
        );
    }

    #[test]
    fn edit_requires_storage() {
        assert!(
            dispatch(Command::Edit {
                id: "abc".to_string(),
                value: "y".to_string(),
            })
            .is_err()
        );
    }

    #[test]
    fn remove_requires_storage() {
        assert!(
            dispatch(Command::Remove {
                id: "abc".to_string(),
            })
            .is_err()
        );
    }
}
