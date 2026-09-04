//! semantic search `patch` execution seam.
//!
//! Grammar: `patch <positive-integer-id> [--text <text>] [--metadata <json>]`.
//! At least one of `--text`/`--metadata` is required (absent payload is a
//! usage error). Metadata is full replacement, not merge; absent
//! `--metadata` retains the stored object. The public ID stays stable: the
//! `records` row is updated in place and a replaced vector reuses the same
//! explicit rowid (evidence-backed delete-plus-insert).
//!
//! Ordering (child spec + task 6.3): validate argv/payload, then open
//! storage, then load the record (record-not-found before any model work),
//! then compute the new identity. An identical patch is a successful no-op
//! (`patched:false`) with zero model work. Otherwise the duplicate/collision
//! check runs before any embedding; metadata-only patches reuse the existing
//! vector (zero embeds); text changes embed outside the write transaction
//! and then apply one atomic transaction. A resulting hash owned by another
//! live record fails operationally without modifying either record.

use std::path::Path;

use crate::dispatch::DispatchOutput;
use crate::error::LibraryError;
use crate::format::OutputFormat;

/// Generic operational failure for the patch path.
///
/// Detail-free like the query/ingest/remove gates: storage paths, SQL,
/// vectors, and model/cache details never reach diagnostics. Non-usage, so
/// the adapter reports status 1 with empty stdout. Usage errors and the
/// distinct record-not-found error pass through unchanged.
fn operational_err() -> LibraryError {
    LibraryError::Io {
        message: "patch failed".to_string(),
    }
}

/// Mutation-store seam for patch orchestration.
///
/// Production adapter is [`SqliteStore`](crate::vec_store::SqliteStore);
/// in-memory fakes prove ordering (duplicate-before-embed, zero embeds on
/// metadata-only/identical patches) without SQLite or a model.
pub(crate) trait MutationStore {
    fn read_for_patch(&self, id: i64) -> Result<(String, String, String), LibraryError>;
    fn find_by_hash(&self, hash: &str) -> Result<Option<i64>, LibraryError>;
    fn apply_patch(
        &mut self,
        id: i64,
        text: &str,
        canonical: &str,
        hash: &str,
        embedding: Option<&[f32]>,
    ) -> Result<(), LibraryError>;
}

impl MutationStore for crate::vec_store::SqliteStore {
    fn read_for_patch(&self, id: i64) -> Result<(String, String, String), LibraryError> {
        Self::read_for_patch(self, id)
    }
    fn find_by_hash(&self, hash: &str) -> Result<Option<i64>, LibraryError> {
        // Same lookup the ingest lane uses (UNIQUE content hash).
        <Self as crate::ingest::HashStore>::find_by_hash(self, hash)
    }
    fn apply_patch(
        &mut self,
        id: i64,
        text: &str,
        canonical: &str,
        hash: &str,
        embedding: Option<&[f32]>,
    ) -> Result<(), LibraryError> {
        Self::apply_patch(self, id, text, canonical, hash, embedding)
    }
}

/// Successful patch outcome: the stable public ID plus whether anything changed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PatchOutcome {
    pub(crate) id: i64,
    pub(crate) patched: bool,
}

/// Render the exact compact success JSON: `{"id":N,"patched":true|false}`.
fn output_json(out: &PatchOutcome) -> String {
    format!("{{\"id\":{},\"patched\":{}}}", out.id, out.patched)
}

/// Pure argv/payload validation shared by every entry point.
///
/// Runs before any storage, model, or embedding work: positive-integer ID,
/// payload presence, ingest text validation for `--text`, and full
/// canonicalization for `--metadata`. Every violation is a usage error.
struct ValidatedPatch {
    id: i64,
    text: Option<String>,
    canonical_meta: Option<String>,
}

fn validate_patch_args(
    id_raw: &str,
    text: Option<&str>,
    metadata_raw: Option<&str>,
) -> Result<ValidatedPatch, LibraryError> {
    // Shared positive-integer rule; the message names the patch verb.
    let id = crate::remove::parse_record_id(id_raw).map_err(|_| LibraryError::InvalidInput {
        message: format!("patch requires a positive integer <ID>, found: {id_raw}"),
    })?;
    if text.is_none() && metadata_raw.is_none() {
        return Err(LibraryError::InvalidInput {
            message: "patch requires --text or --metadata".to_string(),
        });
    }
    if let Some(t) = text {
        crate::metadata::validate_text(t)?;
    }
    let canonical_meta = match metadata_raw {
        None => None,
        Some(raw) => Some(crate::metadata::canonicalize_metadata(Some(raw))?),
    };
    Ok(ValidatedPatch {
        id,
        text: text.map(str::to_string),
        canonical_meta,
    })
}

/// Planned mutation after the record is loaded and the new identity computed.
enum PatchPlan {
    /// Effective content equals the stored content: succeed with `patched:false`.
    NoOp { id: i64 },
    /// Only metadata differs: reuse the existing vector, no embedding.
    MetadataOnly {
        id: i64,
        canonical: String,
        hash: String,
    },
    /// Text differs: the caller must embed, then apply atomically.
    TextChange {
        id: i64,
        text: String,
        canonical: String,
        hash: String,
    },
}

/// Load, hash, no-op-detect, and collision-check. No model work here.
fn plan_patch<S: MutationStore>(
    store: &S,
    validated: &ValidatedPatch,
) -> Result<PatchPlan, LibraryError> {
    // Record-not-found surfaces here, before any model work.
    let (stored_text, stored_canonical, stored_hash) = store.read_for_patch(validated.id)?;
    let effective_text = validated.text.as_deref().unwrap_or(&stored_text);
    let effective_canonical = validated
        .canonical_meta
        .as_deref()
        .unwrap_or(&stored_canonical);
    let new_hash = crate::metadata::compute_identity(effective_canonical, effective_text);
    if new_hash == stored_hash {
        return Ok(PatchPlan::NoOp { id: validated.id });
    }
    // Duplicate/collision rule runs before any embedding: a hash owned by
    // another live record fails without modifying either record.
    if let Some(other) = store.find_by_hash(&new_hash)? {
        if other != validated.id {
            return Err(operational_err());
        }
    }
    let text_changed = effective_text != stored_text;
    if text_changed {
        Ok(PatchPlan::TextChange {
            id: validated.id,
            text: effective_text.to_string(),
            canonical: effective_canonical.to_string(),
            hash: new_hash,
        })
    } else {
        Ok(PatchPlan::MetadataOnly {
            id: validated.id,
            canonical: effective_canonical.to_string(),
            hash: new_hash,
        })
    }
}

/// Execute a validated patch: plan, then (only for real text changes) embed
/// outside any transaction via `embed_one`, then apply one atomic commit.
fn execute_patch<S: MutationStore>(
    store: &mut S,
    validated: &ValidatedPatch,
    embed_one: impl FnOnce(&str) -> Result<Vec<f32>, LibraryError>,
) -> Result<PatchOutcome, LibraryError> {
    match plan_patch(store, validated)? {
        PatchPlan::NoOp { id } => Ok(PatchOutcome { id, patched: false }),
        PatchPlan::MetadataOnly {
            id,
            canonical,
            hash,
        } => {
            // No embedding on this path: the stored vector is reused as-is.
            let (stored_text, _, _) = store.read_for_patch(id)?;
            store
                .apply_patch(id, &stored_text, &canonical, &hash, None)
                .map_err(|_| operational_err())?;
            Ok(PatchOutcome { id, patched: true })
        }
        PatchPlan::TextChange {
            id,
            text,
            canonical,
            hash,
        } => {
            // Model work happens here, outside any SQLite write transaction.
            // Usage errors (e.g. token limits) pass through with status 2;
            // backend failures are redacted to the generic operational error.
            let embedding = match embed_one(&text) {
                Ok(embedding) => embedding,
                Err(e) if e.is_usage() => return Err(e),
                Err(_) => return Err(operational_err()),
            };
            store
                .apply_patch(id, &text, &canonical, &hash, Some(&embedding))
                .map_err(|_| operational_err())?;
            Ok(PatchOutcome { id, patched: true })
        }
    }
}

/// Test seam (unit tests only): validate, plan, and execute against an
/// injected store with an injected single-shot embed closure (called at
/// most once, only for real text changes). The store is provided already
/// open by the caller.
#[cfg(test)]
pub(crate) fn run_in<S: MutationStore>(
    store: &mut S,
    embed_one: impl FnOnce(&str) -> Result<Vec<f32>, LibraryError>,
    id_raw: &str,
    text: Option<&str>,
    metadata_raw: Option<&str>,
) -> Result<PatchOutcome, LibraryError> {
    let validated = validate_patch_args(id_raw, text, metadata_raw)?;
    execute_patch(store, &validated, embed_one)
}

/// Production `patch`: pure validation first, then read-write open without
/// creation (missing storage is operational), then plan/execute. The model
/// cache is ensured lazily inside the embed closure, so identical,
/// metadata-only, and colliding patches perform zero model work.
pub(crate) fn run(
    root: &Path,
    id_raw: &str,
    text: Option<&str>,
    metadata_raw: Option<&str>,
) -> Result<DispatchOutput, LibraryError> {
    // Validate argv/payload before opening storage or touching the model.
    let validated = validate_patch_args(id_raw, text, metadata_raw)?;
    let mut store = match crate::vec_store::SqliteStore::open_existing_rw(root) {
        Ok(store) => store,
        Err(_) => return Err(operational_err()),
    };
    let outcome = execute_patch(&mut store, &validated, |t| {
        let paths = crate::model_cache::cached_model_files().map_err(|_| operational_err())?;
        let embedder = crate::embed::BgeEmbedder::load(&paths).map_err(|_| operational_err())?;
        use crate::ingest::Embedder;
        match embedder.embed(t) {
            Ok(embedding) => Ok(embedding),
            Err(e) if e.is_usage() => Err(e),
            Err(_) => Err(operational_err()),
        }
    })?;
    Ok(DispatchOutput {
        format: OutputFormat::Json,
        body: output_json(&outcome),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::HashMap;

    /// In-memory fake store: records keyed by id with hash index.
    struct MemStore {
        rows: HashMap<i64, (String, String, String)>,
        hash_to_id: HashMap<String, i64>,
        next: i64,
        applies: usize,
        fail_apply: bool,
    }

    impl MemStore {
        fn new() -> Self {
            Self {
                rows: HashMap::new(),
                hash_to_id: HashMap::new(),
                next: 1,
                applies: 0,
                fail_apply: false,
            }
        }

        fn seed(&mut self, text: &str, canonical: &str) -> i64 {
            let hash = crate::metadata::compute_identity(canonical, text);
            let id = self.next;
            self.next += 1;
            self.rows
                .insert(id, (text.to_string(), canonical.to_string(), hash.clone()));
            self.hash_to_id.insert(hash, id);
            id
        }
    }

    impl MutationStore for MemStore {
        fn read_for_patch(&self, id: i64) -> Result<(String, String, String), LibraryError> {
            self.rows
                .get(&id)
                .cloned()
                .ok_or(LibraryError::MissingDefinition { id: id.to_string() })
        }
        fn find_by_hash(&self, hash: &str) -> Result<Option<i64>, LibraryError> {
            Ok(self.hash_to_id.get(hash).copied())
        }
        fn apply_patch(
            &mut self,
            id: i64,
            text: &str,
            canonical: &str,
            hash: &str,
            _embedding: Option<&[f32]>,
        ) -> Result<(), LibraryError> {
            if self.fail_apply {
                return Err(LibraryError::Io {
                    message: "fake commit boom".to_string(),
                });
            }
            let (_, _, old_hash) = self
                .rows
                .get(&id)
                .cloned()
                .ok_or(LibraryError::MissingDefinition { id: id.to_string() })?;
            self.applies += 1;
            self.hash_to_id.remove(&old_hash);
            self.rows.insert(
                id,
                (text.to_string(), canonical.to_string(), hash.to_string()),
            );
            self.hash_to_id.insert(hash.to_string(), id);
            Ok(())
        }
    }

    struct CountingEmbed {
        calls: Cell<usize>,
        seen: std::cell::RefCell<Vec<String>>,
    }

    impl CountingEmbed {
        fn new() -> Self {
            Self {
                calls: Cell::new(0),
                seen: std::cell::RefCell::new(Vec::new()),
            }
        }
        fn closure(&self) -> impl FnOnce(&str) -> Result<Vec<f32>, LibraryError> + '_ {
            |t: &str| {
                self.calls.set(self.calls.get() + 1);
                self.seen.borrow_mut().push(t.to_string());
                Ok(vec![0.25f32; crate::ingest::INGEST_DIMENSION as usize])
            }
        }
    }

    #[test]
    fn absent_payload_is_usage() {
        let mut s = MemStore::new();
        let id = s.seed("hello", "{}");
        let embed = CountingEmbed::new();
        let err = run_in(&mut s, embed.closure(), &id.to_string(), None, None).unwrap_err();
        assert!(err.is_usage());
        assert_eq!(embed.calls.get(), 0);
    }

    #[test]
    fn invalid_id_and_payload_are_usage_with_zero_embeds() {
        let mut s = MemStore::new();
        s.seed("hello", "{}");
        for (id, text, meta) in [
            ("0", Some("x"), None),
            ("-1", Some("x"), None),
            ("abc", Some("x"), None),
            ("1", Some("   "), None),
            ("1", Some("x"), Some("{bad")),
            ("1", Some("x"), Some(r#"{"a":1}"#)),
        ] {
            let embed = CountingEmbed::new();
            let err = run_in(&mut s, embed.closure(), id, text, meta).unwrap_err();
            assert!(err.is_usage(), "args ({id:?}) must be usage, got {err:?}");
            assert_eq!(embed.calls.get(), 0, "no model work on usage rejection");
        }
        assert_eq!(s.applies, 0);
    }

    #[test]
    fn identical_patch_is_noop_with_zero_embeds() {
        let mut s = MemStore::new();
        let id = s.seed("hello", r#"{"a":"1"}"#);
        // Same text + same canonical metadata (reordered keys canonicalize equal).
        let embed = CountingEmbed::new();
        let out = run_in(
            &mut s,
            embed.closure(),
            &id.to_string(),
            Some("hello"),
            Some(r#"{"a":"1"}"#),
        )
        .unwrap();
        assert!(!out.patched);
        assert_eq!(out.id, id);
        assert_eq!(embed.calls.get(), 0);
        assert_eq!(s.applies, 0);
        // Text-only identical repeat is also a no-op.
        let embed = CountingEmbed::new();
        let out = run_in(
            &mut s,
            embed.closure(),
            &id.to_string(),
            Some("hello"),
            None,
        )
        .unwrap();
        assert!(!out.patched);
        assert_eq!(embed.calls.get(), 0);
    }

    #[test]
    fn metadata_only_patch_applies_with_zero_embeds() {
        let mut s = MemStore::new();
        let id = s.seed("hello", r#"{"a":"1"}"#);
        let embed = CountingEmbed::new();
        let out = run_in(
            &mut s,
            embed.closure(),
            &id.to_string(),
            None,
            Some(r#"{"a":"2"}"#),
        )
        .unwrap();
        assert!(out.patched);
        assert_eq!(embed.calls.get(), 0, "metadata-only patch must not embed");
        assert_eq!(s.applies, 1);
        let (text, canonical, _) = s.read_for_patch(id).unwrap();
        assert_eq!(text, "hello");
        assert_eq!(canonical, r#"{"a":"2"}"#);
    }

    #[test]
    fn text_change_embeds_exact_text_once_then_applies() {
        let mut s = MemStore::new();
        let id = s.seed("hello", "{}");
        let embed = CountingEmbed::new();
        let out = run_in(
            &mut s,
            embed.closure(),
            &id.to_string(),
            Some("  hello world  "),
            None,
        )
        .unwrap();
        assert!(out.patched);
        assert_eq!(embed.calls.get(), 1);
        assert_eq!(
            embed.seen.borrow().as_slice(),
            &["  hello world  ".to_string()]
        );
        assert_eq!(s.applies, 1);
        let (text, canonical, _) = s.read_for_patch(id).unwrap();
        assert_eq!(text, "  hello world  ");
        assert_eq!(canonical, "{}");
    }

    #[test]
    fn hash_collision_fails_without_modifying_either_record_or_embedding() {
        let mut s = MemStore::new();
        let id_a = s.seed("alpha", "{}");
        let id_b = s.seed("beta", "{}");
        let before_a = s.read_for_patch(id_a).unwrap();
        let before_b = s.read_for_patch(id_b).unwrap();
        // Patch B toward A's exact content: the resulting hash belongs to A.
        let embed = CountingEmbed::new();
        let err = run_in(
            &mut s,
            embed.closure(),
            &id_b.to_string(),
            Some("alpha"),
            None,
        )
        .unwrap_err();
        assert!(!err.is_usage(), "collision is operational, got {err:?}");
        assert_eq!(embed.calls.get(), 0, "duplicate check precedes embedding");
        assert_eq!(s.applies, 0);
        assert_eq!(s.read_for_patch(id_a).unwrap(), before_a);
        assert_eq!(s.read_for_patch(id_b).unwrap(), before_b);
    }

    #[test]
    fn missing_record_is_not_found_with_zero_embeds() {
        let mut s = MemStore::new();
        s.seed("hello", "{}");
        let embed = CountingEmbed::new();
        let err = run_in(&mut s, embed.closure(), "999", Some("x"), None).unwrap_err();
        assert_eq!(
            err,
            LibraryError::MissingDefinition {
                id: "999".to_string()
            }
        );
        assert_eq!(embed.calls.get(), 0);
    }

    #[test]
    fn commit_failure_is_operational() {
        let mut s = MemStore::new();
        let id = s.seed("hello", "{}");
        s.fail_apply = true;
        let embed = CountingEmbed::new();
        let err = run_in(
            &mut s,
            embed.closure(),
            &id.to_string(),
            Some("world"),
            None,
        )
        .unwrap_err();
        assert!(!err.is_usage());
        assert_eq!(err, operational_err());
    }

    #[test]
    fn production_patch_model_access_is_cached_only() {
        let source = include_str!("patch.rs");
        let production = source
            .split("\n#[cfg(test)]\nmod tests {")
            .next()
            .expect("production patch source");

        assert!(production.contains("cached_model_files"));
        for banned in [
            "ensure_model_files",
            "ensure_with",
            "ureq",
            "fetch(",
            "download",
        ] {
            assert!(
                !production.contains(banned),
                "patch production path must not contain {banned:?}"
            );
        }
    }

    #[test]
    fn output_json_shape_is_exact() {
        assert_eq!(
            output_json(&PatchOutcome {
                id: 1,
                patched: true
            }),
            r#"{"id":1,"patched":true}"#
        );
        assert_eq!(
            output_json(&PatchOutcome {
                id: 2,
                patched: false
            }),
            r#"{"id":2,"patched":false}"#
        );
    }
}
