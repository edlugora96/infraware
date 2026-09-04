//! The single caller-independent dispatch boundary.

use crate::command::Command;
use crate::definition;
use crate::error::LibraryError;
use crate::format::OutputFormat;
use crate::ingest;
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
            // RAG `remove` (parent + child specs): hard delete by positive
            // integer record id across records/meta_kv/vecs. The legacy
            // Markdown definition store owns no `remove` behavior under the
            // approved RAG specs, so every `remove` routes here; ID shape,
            // storage, and record-not-found rules live in `remove::run`.
            crate::remove::run(root, &id)
        }
        Command::Patch {
            id,
            text,
            metadata_raw,
        } => crate::patch::run(root, &id, text.as_deref(), metadata_raw.as_deref()),
        Command::List { format } => {
            let body = definition::list(root, format)?;
            Ok(DispatchOutput { format, body })
        }
        Command::Ingest { text, metadata_raw } => ingest::run(root, &text, metadata_raw.as_deref()),
        // Task 2.3: `query` execution seam. The arm only routes the owned
        // parser payload to `query::run`; behavior stays deferred to tasks
        // 3.x-6.x. The stub returns a non-usage operational error, so `run`
        // maps it to status 1 with empty stdout (never usage status 2).
        Command::Query { text, filter } => crate::query::run(root, &text, &filter),
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
    fn remove_non_integer_id_is_usage_without_touching_storage() {
        // RAG `remove` requires a positive integer id: a non-integer id is
        // a usage error (status 2) and performs no storage work, even on a
        // fresh root with no index and no legacy store.
        let root = temp_root();
        assert_eq!(
            dispatch(
                Command::Remove {
                    id: "abc".to_string(),
                },
                &root.0,
            ),
            Err(LibraryError::InvalidInput {
                message: "remove requires a positive integer <ID>, found: abc".to_string(),
            })
        );
        assert!(!root.0.join(".infraware").exists());
    }

    #[test]
    fn remove_numeric_id_on_missing_storage_is_operational() {
        // A well-shaped id with no index present fails operationally
        // (status 1) and creates no files.
        let root = temp_root();
        let err = dispatch(
            Command::Remove {
                id: "1".to_string(),
            },
            &root.0,
        )
        .unwrap_err();
        assert!(!err.is_usage());
        assert!(!root.0.join(".infraware").exists());
    }

    #[test]
    fn remove_and_patch_route_through_dispatch_on_seeded_index() {
        // End-to-end dispatch coverage over a real index seeded through the
        // storage seam with synthetic embeddings (no model needed): remove
        // emits the exact success JSON, repeat remove is record-not-found,
        // and a metadata-only patch no-ops without model work.
        use crate::ingest::{HashStore, INGEST_DIMENSION};
        let root = temp_root();
        let mut store = crate::vec_store::SqliteStore::open(&root.0).unwrap();
        let emb = vec![0.5f32; INGEST_DIMENSION as usize];
        let canonical = crate::metadata::canonicalize_metadata(None).unwrap();
        let hash = crate::metadata::compute_identity(&canonical, "hello");
        let id = store.insert(&hash, "hello", &canonical, &emb).unwrap();
        drop(store);
        let out =
            dispatch(Command::Remove { id: id.to_string() }, &root.0).expect("remove succeeds");
        assert_eq!(out.body, format!("{{\"id\":{id},\"removed\":true}}"));
        assert_eq!(
            dispatch(Command::Remove { id: id.to_string() }, &root.0,),
            Err(LibraryError::MissingDefinition { id: id.to_string() })
        );
        // Re-seed and exercise the metadata-only patch path through
        // dispatch (no embedding involved, so no model cache is needed).
        let mut store = crate::vec_store::SqliteStore::open(&root.0).unwrap();
        let hash2 = crate::metadata::compute_identity(&canonical, "world");
        let id2 = store.insert(&hash2, "world", &canonical, &emb).unwrap();
        drop(store);
        let out = dispatch(
            Command::Patch {
                id: id2.to_string(),
                text: None,
                metadata_raw: Some("{}".to_string()),
            },
            &root.0,
        )
        .expect("identical patch succeeds");
        assert_eq!(out.body, format!("{{\"id\":{id2},\"patched\":false}}"));
        let out = dispatch(
            Command::Patch {
                id: id2.to_string(),
                text: None,
                metadata_raw: Some(r#"{"a":"1"}"#.to_string()),
            },
            &root.0,
        )
        .expect("metadata patch succeeds");
        assert_eq!(out.body, format!("{{\"id\":{id2},\"patched\":true}}"));
    }

    #[test]
    fn ingest_duplicate_offline_from_prepopulated_store() {
        use crate::ingest::{HashStore, INGEST_DIMENSION};
        let root = temp_root();
        let mut store = crate::vec_store::SqliteStore::open(&root.0).unwrap();
        let canonical = crate::metadata::canonicalize_metadata(None).unwrap();
        let hash = crate::metadata::compute_identity(&canonical, "hello");
        let emb = vec![0.5f32; INGEST_DIMENSION as usize];
        let id = store.insert(&hash, "hello", &canonical, &emb).unwrap();
        drop(store);
        let out = dispatch(
            Command::Ingest {
                text: "hello".to_string(),
                metadata_raw: None,
            },
            &root.0,
        )
        .expect("duplicate ingest succeeds");
        let v: serde_json::Value = serde_json::from_str(&out.body).unwrap();
        assert_eq!(v["id"], id);
        assert_eq!(v["duplicate"], true);
        assert_eq!(v.as_object().unwrap().len(), 5);
    }

    #[test]
    fn ingest_invalid_input_status() {
        let root = temp_root();
        let err = dispatch(
            Command::Ingest {
                text: "   ".to_string(),
                metadata_raw: None,
            },
            &root.0,
        )
        .unwrap_err();
        assert!(err.is_usage());
        assert!(matches!(err, LibraryError::InvalidInput { .. }));
    }

    // ---- task 2.3: `query` dispatch arm ----

    #[test]
    fn query_dispatch_routes_to_query_seam_as_operational() {
        // The arm preserves the owned parser payload and delegates to the
        // query seam; the storage gate reports a generic non-usage
        // operational error so `run` maps it to status 1 with empty stdout.
        use std::collections::BTreeMap;
        let root = temp_root();
        let filter: BTreeMap<String, String> =
            [("a".to_string(), "1".to_string())].into_iter().collect();
        let err = dispatch(
            Command::Query {
                text: "  hello world  ".to_string(),
                filter,
            },
            &root.0,
        )
        .unwrap_err();
        assert!(!err.is_usage());
        assert_eq!(
            err,
            LibraryError::Io {
                message: crate::query::QUERY_OPERATIONAL_MESSAGE.to_string(),
            }
        );
    }

    #[test]
    fn query_dispatch_leaves_existing_commands_unchanged() {
        // Regression: wiring the `query` arm must not alter any existing
        // command behavior or the stream/status ownership in `run`.
        let root = temp_root();
        let out = dispatch(Command::Init, &root.0).expect("init succeeds");
        assert!(out.body.is_empty());
        assert_eq!(
            dispatch(
                Command::Get {
                    id: "abc".to_string(),
                    format: OutputFormat::Markdown,
                },
                &root.0,
            ),
            Err(LibraryError::MissingDefinition {
                id: "abc".to_string()
            })
        );
    }
}
