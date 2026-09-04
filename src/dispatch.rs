//! The single caller-independent dispatch boundary.

use crate::command::Command;
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
/// Exactly the six SQLite-backed verbs are routed, each to its thin core:
/// - `ingest`/`query` keep their existing SQLite paths.
/// - `list`/`get` read through the approved read-only snapshot lane.
/// - `patch`/`remove` mutate through the approved read-write lane.
pub fn dispatch(command: Command, root: &Path) -> Result<DispatchOutput, LibraryError> {
    match command {
        Command::Ingest { text, metadata_raw } => ingest::run(root, &text, metadata_raw.as_deref()),
        // Task 2.3: `query` execution seam. The arm only routes the owned
        // parser payload to `query::run`; behavior stays deferred to tasks
        // 3.x-6.x. The stub returns a non-usage operational error, so `run`
        // maps it to status 1 with empty stdout (never usage status 2).
        Command::Query { text, filter } => crate::query::run(root, &text, &filter),
        // Read-only SQLite verbs through the thin records cores: direct
        // point fetch (`get`) and ascending-id snapshot scan (`list`).
        Command::List {
            filter,
            limit,
            offset,
        } => crate::records::list(root, &filter, limit, offset),
        Command::Get { id } => crate::records::get(root, &id),
        Command::Patch {
            id,
            text,
            metadata_raw,
        } => crate::patch::run(root, &id, text.as_deref(), metadata_raw.as_deref()),
        Command::Remove { id } => {
            // RAG `remove` (parent + child specs): hard delete by positive
            // integer record id across records/meta_kv/vecs. ID shape,
            // storage, and record-not-found rules live in `remove::run`.
            crate::remove::run(root, &id)
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
    fn all_six_verbs_route_through_dispatch() {
        // Each of the six verbs reaches its core function through the
        // single dispatch seam. Seeded-index behavior for remove/patch/
        // get/list is covered below; here every verb at least routes
        // without an unknown-command rejection on a fresh root.
        use std::collections::BTreeMap;
        let root = temp_root();
        // ingest: blank text is a usage error from the core (routed, not unknown).
        assert!(
            dispatch(
                Command::Ingest {
                    text: "   ".to_string(),
                    metadata_raw: None,
                },
                &root.0,
            )
            .unwrap_err()
            .is_usage()
        );
        // query: missing index is operational (routed, never usage).
        assert!(
            !dispatch(
                Command::Query {
                    text: "hello".to_string(),
                    filter: BTreeMap::new(),
                },
                &root.0,
            )
            .unwrap_err()
            .is_usage()
        );
        // list/get: missing index is operational (routed, never usage).
        assert!(
            !dispatch(
                Command::List {
                    filter: Default::default(),
                    limit: 50,
                    offset: 0,
                },
                &root.0,
            )
            .unwrap_err()
            .is_usage()
        );
        assert!(
            !dispatch(
                Command::Get {
                    id: "1".to_string(),
                },
                &root.0,
            )
            .unwrap_err()
            .is_usage()
        );
        // patch/remove: malformed ids are usage errors from the cores.
        assert!(
            dispatch(
                Command::Patch {
                    id: "abc".to_string(),
                    text: Some("x".to_string()),
                    metadata_raw: None,
                },
                &root.0,
            )
            .unwrap_err()
            .is_usage()
        );
        assert!(
            dispatch(
                Command::Remove {
                    id: "abc".to_string(),
                },
                &root.0,
            )
            .unwrap_err()
            .is_usage()
        );
        assert!(!root.0.join(".infraware").exists());
    }

    #[test]
    fn get_and_list_on_missing_store_are_operational() {
        // A missing index fails closed (status 1 shape) and creates no
        // files; the snapshot lane never creates storage on the read path.
        let root = temp_root();
        for err in [
            dispatch(
                Command::List {
                    filter: Default::default(),
                    limit: 50,
                    offset: 0,
                },
                &root.0,
            )
            .unwrap_err(),
            dispatch(
                Command::Get {
                    id: "1".to_string(),
                },
                &root.0,
            )
            .unwrap_err(),
        ] {
            assert!(!err.is_usage());
        }
        assert!(!root.0.join(".infraware").exists());
    }

    #[test]
    fn get_validates_id_before_storage() {
        // A non-integer id is a usage error (status 2) with no storage work.
        let root = temp_root();
        assert_eq!(
            dispatch(
                Command::Get {
                    id: "abc".to_string(),
                },
                &root.0,
            ),
            Err(LibraryError::InvalidInput {
                message: "get requires a positive integer <ID>, found: abc".to_string(),
            })
        );
        assert!(!root.0.join(".infraware").exists());
    }

    #[test]
    fn get_and_list_round_trip_seeded_index() {
        // End-to-end dispatch coverage for the read verbs over a real index
        // seeded through the storage seam (no model needed): get returns
        // the stored record, list returns it in ascending-id order with a
        // snapshot-consistent total.
        use crate::ingest::{HashStore, INGEST_DIMENSION};
        let root = temp_root();
        let mut store = crate::vec_store::SqliteStore::open(&root.0).unwrap();
        let emb = vec![0.5f32; INGEST_DIMENSION as usize];
        let canonical = crate::metadata::canonicalize_metadata(None).unwrap();
        let hash = crate::metadata::compute_identity(&canonical, "hello");
        let id = store.insert(&hash, "hello", &canonical, &emb).unwrap();
        drop(store);
        let out = dispatch(Command::Get { id: id.to_string() }, &root.0).expect("get succeeds");
        assert_eq!(out.format, crate::format::OutputFormat::Json);
        let v: serde_json::Value = serde_json::from_str(&out.body).unwrap();
        assert_eq!(v["id"], id);
        assert_eq!(v["text"], serde_json::json!("hello"));
        // Missing record is the distinct record-not-found error.
        assert_eq!(
            dispatch(
                Command::Get {
                    id: "999".to_string(),
                },
                &root.0,
            ),
            Err(LibraryError::MissingDefinition {
                id: "999".to_string()
            })
        );
        let out = dispatch(
            Command::List {
                filter: Default::default(),
                limit: 50,
                offset: 0,
            },
            &root.0,
        )
        .expect("list succeeds");
        let v: serde_json::Value = serde_json::from_str(&out.body).unwrap();
        assert_eq!(v["total"], 1);
        assert_eq!(v["records"].as_array().unwrap().len(), 1);
        assert_eq!(v["records"][0]["id"], id);
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
        assert!(
            !dispatch(
                Command::List {
                    filter: Default::default(),
                    limit: 50,
                    offset: 0,
                },
                &root.0,
            )
            .unwrap_err()
            .is_usage()
        );
        assert_eq!(
            dispatch(
                Command::Get {
                    id: "abc".to_string(),
                },
                &root.0,
            ),
            Err(LibraryError::InvalidInput {
                message: "get requires a positive integer <ID>, found: abc".to_string(),
            })
        );
    }
}
