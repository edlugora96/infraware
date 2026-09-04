//! Atomic ingest write path. This is the only fact creator.
//!
//! One call ingests exactly one fact/query pair, with one `<TEXT>` and an
//! optional `[<METADATA-JSON>]`, in a single SQLite transaction. The whole
//! operation fails if any step fails. The exact-argument parser rejects batch
//! and legacy verb/flag forms before they reach this seam.

use crate::dispatch::DispatchOutput;
use crate::error::LibraryError;
use crate::format::OutputFormat;

#[allow(dead_code, unreachable_pub)]
pub const INGEST_MODEL: &str = "BAAI/bge-small-en-v1.5";
#[allow(dead_code, unreachable_pub)]
pub const INGEST_MODEL_REVISION: &str = "5c38ec7c405ec4b44b94cc5a9bb96e735b38267a";
#[allow(dead_code, unreachable_pub)]
pub const INGEST_DIMENSION: u32 = 384;

#[allow(dead_code, unreachable_pub)]
#[derive(Debug)]
pub struct IngestOutput {
    pub id: i64,
    pub duplicate: bool,
    pub metadata_canonical: String,
    pub model: &'static str,
    pub dimension: u32,
}

#[allow(dead_code, unreachable_pub)]
pub fn output_json(out: &IngestOutput) -> String {
    let metadata: serde_json::Value =
        serde_json::from_str(&out.metadata_canonical).unwrap_or(serde_json::Value::Null);
    let metadata_s = serde_json::to_string(&metadata).unwrap_or_else(|_| "null".to_string());
    format!(
        "{{\"id\":{},\"duplicate\":{},\"metadata\":{},\"model\":{:?},\"dimension\":{}}}",
        out.id, out.duplicate, metadata_s, out.model, out.dimension
    )
}

#[allow(dead_code, unreachable_pub)]
pub trait HashStore {
    fn find_by_hash(&self, hash: &str) -> Result<Option<i64>, LibraryError>;
    fn insert(
        &mut self,
        hash: &str,
        text: &str,
        canonical: &str,
        embedding: &[f32],
    ) -> Result<i64, LibraryError>;
}

#[allow(dead_code, unreachable_pub)]
pub trait Embedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, LibraryError>;
}

#[allow(dead_code, unreachable_pub)]
pub fn run_in<S: HashStore, E: Embedder>(
    store: &mut S,
    embedder: &E,
    text: &str,
    metadata_raw: Option<&str>,
) -> Result<IngestOutput, LibraryError> {
    crate::metadata::validate_text(text)?;
    let canonical = crate::metadata::canonicalize_metadata(metadata_raw)?;
    let hash = crate::metadata::compute_identity(&canonical, text);
    if let Some(id) = store.find_by_hash(&hash)? {
        return Ok(IngestOutput {
            id,
            duplicate: true,
            metadata_canonical: canonical,
            model: INGEST_MODEL,
            dimension: INGEST_DIMENSION,
        });
    }
    let embedding = embedder.embed(text)?;
    let id = store.insert(&hash, text, &canonical, &embedding)?;
    Ok(IngestOutput {
        id,
        duplicate: false,
        metadata_canonical: canonical,
        model: INGEST_MODEL,
        dimension: INGEST_DIMENSION,
    })
}

/// Generic operational failure for the ingest model stages (task 5.2).
///
/// Detail-free like the query gate: cache/model paths, download details,
/// and vectors never reach diagnostics. Validation errors keep their
/// specific messages; only download/load/embed map here. Non-usage, so
/// the adapter reports status 1 with empty stdout.
fn operational_err() -> LibraryError {
    LibraryError::Io {
        message: "ingest failed".to_string(),
    }
}

/// Production ingest: validate -> canonicalize -> hash -> open store ->
/// duplicate lookup; on miss ensure model, embed (outside transaction),
/// then exactly one insert. Emits compact 5-key JSON.
pub(crate) fn run(
    root: &std::path::Path,
    text: &str,
    metadata_raw: Option<&str>,
) -> Result<DispatchOutput, LibraryError> {
    let (canonical, hash) = crate::metadata::prepare(text, metadata_raw)?;
    let mut store = crate::vec_store::SqliteStore::open(root)?;
    if let Some(id) = store.find_by_hash(&hash)? {
        let out = IngestOutput {
            id,
            duplicate: true,
            metadata_canonical: canonical,
            model: INGEST_MODEL,
            dimension: INGEST_DIMENSION,
        };
        return Ok(DispatchOutput {
            format: OutputFormat::Json,
            body: output_json(&out),
        });
    }
    let paths = crate::model_cache::ensure_model_files().map_err(|_| operational_err())?;
    let embedder = crate::embed::BgeEmbedder::load(&paths).map_err(|_| operational_err())?;
    let embedding = embedder.embed(text).map_err(|_| operational_err())?;
    let id = store.insert(&hash, text, &canonical, &embedding)?;
    let out = IngestOutput {
        id,
        duplicate: false,
        metadata_canonical: canonical,
        model: INGEST_MODEL,
        dimension: INGEST_DIMENSION,
    };
    Ok(DispatchOutput {
        format: OutputFormat::Json,
        body: output_json(&out),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::HashMap;

    struct MemStore {
        map: HashMap<String, i64>,
        next: i64,
        inserts: usize,
        lookups: Cell<usize>,
        rows: usize,
    }

    impl MemStore {
        fn new() -> Self {
            Self {
                map: HashMap::new(),
                next: 1,
                inserts: 0,
                lookups: Cell::new(0),
                rows: 0,
            }
        }
    }

    impl HashStore for MemStore {
        fn find_by_hash(&self, hash: &str) -> Result<Option<i64>, LibraryError> {
            self.lookups.set(self.lookups.get() + 1);
            Ok(self.map.get(hash).copied())
        }
        fn insert(
            &mut self,
            hash: &str,
            _text: &str,
            _canonical: &str,
            _embedding: &[f32],
        ) -> Result<i64, LibraryError> {
            self.inserts += 1;
            self.rows += 1;
            let id = self.next;
            self.next += 1;
            self.map.insert(hash.to_string(), id);
            Ok(id)
        }
    }

    struct CountingEmbedder {
        calls: Cell<usize>,
    }

    impl Embedder for CountingEmbedder {
        fn embed(&self, _text: &str) -> Result<Vec<f32>, LibraryError> {
            self.calls.set(self.calls.get() + 1);
            Ok(vec![0.0; INGEST_DIMENSION as usize])
        }
    }

    #[test]
    fn fresh_insert() {
        let mut s = MemStore::new();
        let e = CountingEmbedder {
            calls: Cell::new(0),
        };
        let out = run_in(&mut s, &e, "hello", None).unwrap();
        assert!(!out.duplicate);
        assert_eq!(out.metadata_canonical, "{}");
        assert_eq!(out.model, INGEST_MODEL);
        assert_eq!(out.dimension, INGEST_DIMENSION);
        assert_eq!(e.calls.get(), 1);
        assert_eq!(s.inserts, 1);
    }

    #[test]
    fn same_id_duplicate_no_embed_no_insert() {
        let mut s = MemStore::new();
        let e = CountingEmbedder {
            calls: Cell::new(0),
        };
        let first = run_in(&mut s, &e, "hello", None).unwrap();
        let rows = s.rows;
        let out = run_in(&mut s, &e, "hello", None).unwrap();
        assert_eq!(out.id, first.id);
        assert!(out.duplicate);
        assert_eq!(e.calls.get(), 1);
        assert_eq!(s.inserts, 1);
        assert_eq!(s.rows, rows);
    }

    #[test]
    fn reordered_keys_same_id() {
        let mut s = MemStore::new();
        let e = CountingEmbedder {
            calls: Cell::new(0),
        };
        let a = run_in(&mut s, &e, "t", Some(r#"{"a":"1","b":"2"}"#)).unwrap();
        let b = run_in(&mut s, &e, "t", Some(r#"{"b":"2","a":"1"}"#)).unwrap();
        assert_eq!(a.id, b.id);
        assert!(b.duplicate);
    }

    #[test]
    fn absent_vs_empty_object_same_id() {
        let mut s = MemStore::new();
        let e = CountingEmbedder {
            calls: Cell::new(0),
        };
        let a = run_in(&mut s, &e, "t", None).unwrap();
        let b = run_in(&mut s, &e, "t", Some("{}")).unwrap();
        assert_eq!(a.id, b.id);
    }

    #[test]
    fn changed_text_or_meta_new_ids() {
        let mut s = MemStore::new();
        let e = CountingEmbedder {
            calls: Cell::new(0),
        };
        let a = run_in(&mut s, &e, "t1", None).unwrap();
        let b = run_in(&mut s, &e, "t2", None).unwrap();
        assert_ne!(a.id, b.id);
        let c = run_in(&mut s, &e, "t1", Some(r#"{"a":"1"}"#)).unwrap();
        assert_ne!(a.id, c.id);
    }

    #[test]
    fn output_json_five_keys() {
        let out = IngestOutput {
            id: 7,
            duplicate: false,
            metadata_canonical: "{}".to_string(),
            model: INGEST_MODEL,
            dimension: INGEST_DIMENSION,
        };
        let j: serde_json::Value = serde_json::from_str(&output_json(&out)).unwrap();
        let obj = j.as_object().unwrap();
        assert_eq!(obj.len(), 5);
        assert!(obj.contains_key("id"));
        assert!(obj.contains_key("duplicate"));
        assert!(obj.contains_key("metadata"));
        assert!(obj.contains_key("model"));
        assert!(obj.contains_key("dimension"));
        assert!(obj.get("metadata").unwrap().is_object());
    }

    #[test]
    fn validation_before_lookup() {
        let mut s = MemStore::new();
        let e = CountingEmbedder {
            calls: Cell::new(0),
        };
        assert!(run_in(&mut s, &e, "", None).is_err());
        assert!(run_in(&mut s, &e, "t", Some("{oops")).is_err());
        assert_eq!(s.lookups.get(), 0);
        assert_eq!(e.calls.get(), 0);
        assert_eq!(s.inserts, 0);
    }

    /// Task 3.4 ordering: duplicate lookup precedes all model work,
    /// embedding completes before insert/transaction, and the duplicate
    /// path emits no model/embed/insert events. The shared event log
    /// stands in for every model stage (cache/load/embed), which the
    /// injected embedder represents on this seam.
    #[test]
    fn ordering_lookup_before_embed_before_insert_with_no_model_work_on_duplicate() {
        use std::cell::RefCell;
        use std::rc::Rc;

        struct LogStore {
            map: HashMap<String, i64>,
            next: i64,
            log: Rc<RefCell<Vec<&'static str>>>,
        }
        impl HashStore for LogStore {
            fn find_by_hash(&self, hash: &str) -> Result<Option<i64>, LibraryError> {
                self.log.borrow_mut().push("lookup");
                Ok(self.map.get(hash).copied())
            }
            fn insert(
                &mut self,
                hash: &str,
                _text: &str,
                _canonical: &str,
                embedding: &[f32],
            ) -> Result<i64, LibraryError> {
                self.log.borrow_mut().push("insert");
                // Insert receives a fully computed embedding, so the
                // embed event must already be logged (outside transaction).
                assert_eq!(embedding.len(), INGEST_DIMENSION as usize);
                let id = self.next;
                self.next += 1;
                self.map.insert(hash.to_string(), id);
                Ok(id)
            }
        }
        struct LogEmbedder {
            log: Rc<RefCell<Vec<&'static str>>>,
        }
        impl Embedder for LogEmbedder {
            fn embed(&self, _text: &str) -> Result<Vec<f32>, LibraryError> {
                self.log.borrow_mut().push("embed");
                Ok(vec![0.0; INGEST_DIMENSION as usize])
            }
        }

        let log: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
        let mut store = LogStore {
            map: HashMap::new(),
            next: 1,
            log: Rc::clone(&log),
        };
        let embedder = LogEmbedder {
            log: Rc::clone(&log),
        };
        // Fresh insert: lookup, then embed, then insert.
        let first = run_in(&mut store, &embedder, "hello", None).unwrap();
        assert!(!first.duplicate);
        assert_eq!(*log.borrow(), vec!["lookup", "embed", "insert"]);
        // Duplicate: lookup only — no model/embed/insert events.
        log.borrow_mut().clear();
        let second = run_in(&mut store, &embedder, "hello", None).unwrap();
        assert!(second.duplicate);
        assert_eq!(second.id, first.id);
        assert_eq!(*log.borrow(), vec!["lookup"]);
    }
}
