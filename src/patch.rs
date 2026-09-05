//! semantic search `patch` execution seam.
//!
//! Grammar: `patch <positive-integer-id> --text <text>`.
//! The public ID stays stable while its text and vector are replaced.
//!
//! Ordering: validate arguments, open storage, load the record before model
//! work, then embed changed text outside the write transaction and apply one
//! atomic update. An identical patch is a successful no-op (`patched:false`).

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
/// in-memory fakes prove ordering and zero embeds for identical patches without
/// SQLite or a model.
pub(crate) trait MutationStore {
    fn read_fact_text(&self, id: i64) -> Result<String, LibraryError>;
    fn apply_patch(
        &mut self,
        id: i64,
        text: &str,
        embedding: Option<&[f32]>,
    ) -> Result<(), LibraryError>;
}
impl MutationStore for crate::vec_store::SqliteStore {
    fn read_fact_text(&self, id: i64) -> Result<String, LibraryError> {
        crate::vec_store::SqliteStore::read_fact_text(self, id)
    }
    fn apply_patch(
        &mut self,
        id: i64,
        text: &str,
        embedding: Option<&[f32]>,
    ) -> Result<(), LibraryError> {
        crate::vec_store::SqliteStore::apply_patch(self, id, text, embedding)
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

struct ValidatedPatch {
    id: i64,
    text: String,
}
fn validate_patch_args(
    id_raw: &str,
    text: Option<&str>,
    _unsupported: Option<&str>,
) -> Result<ValidatedPatch, LibraryError> {
    let id = crate::remove::parse_record_id(id_raw).map_err(|_| LibraryError::InvalidInput {
        message: format!("patch requires a positive integer <ID>, found: {id_raw}"),
    })?;
    let Some(text) = text else {
        return Err(LibraryError::InvalidInput {
            message: "patch requires --text".into(),
        });
    };
    if _unsupported.is_some() {
        return Err(LibraryError::InvalidInput {
            message: "unknown patch option".into(),
        });
    }
    if text.trim().is_empty() {
        return Err(LibraryError::InvalidInput {
            message: "patch text must not be empty".into(),
        });
    }
    Ok(ValidatedPatch {
        id,
        text: text.into(),
    })
}
enum PatchPlan {
    NoOp(i64),
    Change(i64, String),
}
fn plan_patch<S: MutationStore>(s: &S, v: &ValidatedPatch) -> Result<PatchPlan, LibraryError> {
    let old = s.read_fact_text(v.id)?;
    if old == v.text {
        Ok(PatchPlan::NoOp(v.id))
    } else {
        Ok(PatchPlan::Change(v.id, v.text.clone()))
    }
}
fn execute_patch<S: MutationStore>(
    s: &mut S,
    v: &ValidatedPatch,
    embed: impl FnOnce(&str) -> Result<Vec<f32>, LibraryError>,
) -> Result<PatchOutcome, LibraryError> {
    match plan_patch(s, v)? {
        PatchPlan::NoOp(id) => Ok(PatchOutcome { id, patched: false }),
        PatchPlan::Change(id, text) => {
            let e = embed(&text).map_err(|x| if x.is_usage() { x } else { operational_err() })?;
            s.apply_patch(id, &text, Some(&e))
                .map_err(|_| operational_err())?;
            Ok(PatchOutcome { id, patched: true })
        }
    }
}

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
/// cache is ensured lazily inside the embed closure, so identical patches
/// perform zero model work.
pub(crate) fn run(
    root: &Path,
    id_raw: &str,
    text: Option<&str>,
) -> Result<DispatchOutput, LibraryError> {
    // Validate argv/payload before opening storage or touching the model.
    let validated = validate_patch_args(id_raw, text, None)?;
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

    /// In-memory fake store: records keyed by id with text only.
    struct MemStore {
        rows: HashMap<i64, String>,
        next: i64,
        applies: usize,
        fail_apply: bool,
    }

    impl MemStore {
        fn new() -> Self {
            Self {
                rows: HashMap::new(),
                next: 1,
                applies: 0,
                fail_apply: false,
            }
        }

        fn seed(&mut self, text: &str) -> i64 {
            let id = self.next;
            self.next += 1;
            self.rows.insert(id, text.to_string());
            id
        }
    }

    impl MutationStore for MemStore {
        fn read_fact_text(&self, id: i64) -> Result<String, LibraryError> {
            self.rows
                .get(&id)
                .cloned()
                .ok_or(LibraryError::MissingDefinition { id: id.to_string() })
        }

        fn apply_patch(
            &mut self,
            id: i64,
            text: &str,
            _embedding: Option<&[f32]>,
        ) -> Result<(), LibraryError> {
            if self.fail_apply {
                return Err(LibraryError::Io {
                    message: "fake commit boom".to_string(),
                });
            }
            let row = self
                .rows
                .get_mut(&id)
                .ok_or(LibraryError::MissingDefinition { id: id.to_string() })?;
            self.applies += 1;
            *row = text.to_string();
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
        let id = s.seed("hello");
        let embed = CountingEmbed::new();
        let err = run_in(&mut s, embed.closure(), &id.to_string(), None, None).unwrap_err();
        assert!(err.is_usage());
        assert_eq!(embed.calls.get(), 0);
    }

    #[test]
    fn invalid_id_and_payload_are_usage_with_zero_embeds() {
        let mut s = MemStore::new();
        s.seed("hello");
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
        let id = s.seed("hello");
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
    fn text_change_embeds_exact_text_once_then_applies() {
        let mut s = MemStore::new();
        let id = s.seed("hello");
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
        assert_eq!(s.read_fact_text(id).unwrap(), "  hello world  ");
    }

    #[test]
    fn missing_record_is_not_found_with_zero_embeds() {
        let mut s = MemStore::new();
        s.seed("hello");
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
        let id = s.seed("hello");
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
