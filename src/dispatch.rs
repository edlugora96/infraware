//! The single caller-independent dispatch boundary.

use crate::command::Command;
use crate::definition;
use crate::error::LibraryError;
use crate::format::OutputFormat;
use crate::lifecycle;
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
/// The frozen stream/status contract is preserved:
/// - `init` creates `.infraware/ARCHITECTURE.md` via the lifecycle/persistence
///   boundary and succeeds with no structured body (no stdout).
/// - `analyze` succeeds with an empty body: empty/incomplete analysis is
///   successful and finalized by Plan 4.
/// - `get`/`insert`/`edit`/`remove` require canonical storage that does not
///   exist yet, so they return the corresponding typed error (non-zero).
pub fn dispatch(command: Command, root: &Path) -> Result<DispatchOutput, LibraryError> {
    match command {
        Command::Init => {
            // Functional `init`: create `.infraware/ARCHITECTURE.md` only when
            // absent (idempotent, no-clobber). Errors propagate as a typed
            // `LibraryError` and are mapped to stderr + non-zero status by `run`,
            // never to stdout.
            lifecycle::init(root)?;
            Ok(DispatchOutput {
                format: OutputFormat::Markdown,
                body: String::new(),
            })
        }
        Command::Analyze {
            format,
            suggest,
            details,
        } => {
            // Realize the deferred `analyze` behavior (Plan 4 + MVP2): offline,
            // deterministic, read-only scan rendered in `format`. Empty/
            // incomplete/partially-unreadable repos still succeed with a well-
            // formed report and status 0 (design D8). `details` gates evidence.
            let body = crate::analyze::run(root, format, suggest, details)?;
            Ok(DispatchOutput { format, body })
        }
        // Storage-backed definition operations (Plan 3). Each loads the
        // canonical document, enforces strict existence semantics, and persists
        // only through the Plan 2 atomic boundary. A missing canonical file
        // surfaces as UninitializedStorage (never implicit init).
        Command::Get { id, format } => {
            let body = definition::get(root, &id, format)?;
            Ok(DispatchOutput { format, body })
        }
        Command::Insert { id, value } => {
            definition::insert(root, &id, &value)?;
            Ok(DispatchOutput {
                format: OutputFormat::Markdown,
                body: String::new(),
            })
        }
        Command::Edit { id, value } => {
            definition::edit(root, &id, &value)?;
            Ok(DispatchOutput {
                format: OutputFormat::Markdown,
                body: String::new(),
            })
        }
        Command::Remove { id } => {
            definition::remove(root, &id)?;
            Ok(DispatchOutput {
                format: OutputFormat::Markdown,
                body: String::new(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::Command;
    use std::fs;

    /// RAII guard that removes its temp directory on drop, even on panic.
    struct TempRoot(std::path::PathBuf);

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Create a unique, empty temp directory owned by the returned guard.
    fn temp_root() -> TempRoot {
        let base = std::env::temp_dir().join(format!(
            "infraware-dispatch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).expect("create temp root");
        TempRoot(base)
    }

    #[test]
    fn init_creates_storage_with_empty_body() {
        let root = temp_root();
        let out = dispatch(Command::Init, &root.0).expect("init succeeds");
        assert_eq!(out.format, OutputFormat::Markdown);
        assert!(out.body.is_empty(), "init emits no structured body");

        let path = crate::persistence::canonical_path(&root.0);
        assert!(path.exists(), "ARCHITECTURE.md created by dispatch");
    }

    #[test]
    fn init_is_idempotent_through_dispatch() {
        let root = temp_root();
        dispatch(Command::Init, &root.0).expect("first init succeeds");
        let path = crate::persistence::canonical_path(&root.0);
        let first = fs::read_to_string(&path).unwrap();

        // Simulate an external edit; a second dispatch must NOT overwrite it.
        let edited = format!("{first}# external edit\n");
        fs::write(&path, &edited).unwrap();

        dispatch(Command::Init, &root.0).expect("second init succeeds");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            edited,
            "existing content must be left untouched (no clobber)"
        );
    }

    #[test]
    fn init_propagates_io_error() {
        // A root that is itself a file cannot host `.infraware/`, so persistence
        // returns a typed `Io` error that dispatch propagates unchanged.
        let file = std::env::temp_dir().join(format!(
            "infraware-dispatch-file-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&file, b"not a directory").unwrap();
        let result = dispatch(Command::Init, &file);
        let _ = fs::remove_file(&file);

        match result {
            Err(LibraryError::Io { .. }) => {}
            other => panic!("expected Io error, got {other:?}"),
        }
    }

    #[test]
    fn analyze_empty_is_success() {
        // Empty/incomplete analysis still succeeds (status 0) but now yields a
        // well-formed, non-empty report body with all category keys present.
        let out = dispatch(
            Command::Analyze {
                format: OutputFormat::Json,
                suggest: true,
                details: false,
            },
            Path::new("."),
        )
        .expect("empty analyze succeeds");
        assert_eq!(out.format, OutputFormat::Json);
        assert!(
            !out.body.is_empty(),
            "analyze now emits a structured report body"
        );
        assert!(out.body.contains("\"languages\""));
        assert!(out.body.contains("\"suggestions\""));
    }

    #[test]
    fn get_is_missing_definition_in_uninitialized_store() {
        // With no canonical storage, get reports UninitializedStorage (no
        // implicit init), not MissingDefinition.
        let root = temp_root();
        assert_eq!(
            dispatch(
                Command::Get {
                    id: "abc".to_string(),
                    format: OutputFormat::Markdown,
                },
                &root.0,
            ),
            Err(LibraryError::UninitializedStorage)
        );
    }

    #[test]
    fn insert_requires_storage() {
        let root = temp_root();
        assert_eq!(
            dispatch(
                Command::Insert {
                    id: "abc".to_string(),
                    value: "x".to_string(),
                },
                &root.0,
            ),
            Err(LibraryError::UninitializedStorage)
        );
    }

    #[test]
    fn edit_requires_storage() {
        let root = temp_root();
        assert_eq!(
            dispatch(
                Command::Edit {
                    id: "abc".to_string(),
                    value: "y".to_string(),
                },
                &root.0,
            ),
            Err(LibraryError::UninitializedStorage)
        );
    }

    #[test]
    fn remove_requires_storage() {
        let root = temp_root();
        assert_eq!(
            dispatch(
                Command::Remove {
                    id: "abc".to_string(),
                },
                &root.0,
            ),
            Err(LibraryError::UninitializedStorage)
        );
    }
}
