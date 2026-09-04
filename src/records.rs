//! Thin SQLite cores for the `get` and `list` verbs.
//!
//! Reached exclusively through [`dispatch`](crate::dispatch::dispatch). Both
//! verbs are read-only: they open the index through the approved read-only
//! snapshot constructor (never creating or repairing storage) and reuse the
//! existing [`SqliteStore`](crate::vec_store::SqliteStore) read APIs.
//!
//! - `get <ID>` is a direct point fetch by positive integer record id; a
//!   missing record surfaces the distinct record-not-found error.
//! - `list` returns matching records ordered by ascending id with a
//!   snapshot-consistent `total` (full filtered count) plus `limit`
//!   (default 50, clamped to max 500) and `offset` (default 0) paging.
//!
//! Storage failures (missing, unreadable, or incompatible index) map to a
//! single detail-free operational error so the adapter reports status 1 with
//! empty stdout and one `error:` line on stderr.

use std::collections::BTreeMap;
use std::path::Path;

use crate::dispatch::DispatchOutput;
use crate::error::LibraryError;
use crate::format::OutputFormat;

/// Generic operational failure for the get/list read paths.
///
/// Detail-free like the query/ingest/remove gates: storage paths, SQL, and
/// record contents never reach diagnostics. Non-usage, so the adapter
/// reports status 1 with empty stdout. The distinct record-not-found error
/// passes through unchanged.
fn operational_err() -> LibraryError {
    LibraryError::Io {
        message: "store unavailable".to_string(),
    }
}

/// Successful `get` record: stable id, full text bytes, canonical metadata,
/// and creation timestamp.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FetchedRecord {
    pub(crate) id: i64,
    pub(crate) text: String,
    pub(crate) metadata: BTreeMap<String, String>,
    pub(crate) created_at: i64,
}

/// Render the exact compact `get` JSON:
/// `{"id":N,"text":...,"metadata":{...},"created_at":N}`.
///
/// Field order is fixed (`id`, `text`, `metadata`, `created_at`); the text is
/// preserved byte-exactly and metadata serializes in byte (key) order.
fn output_get_json(record: &FetchedRecord) -> String {
    let metadata = serde_json::to_string(&record.metadata)
        .expect("canonical metadata map serialization must succeed");
    let text = serde_json::to_string(&record.text).expect("record text serialization must succeed");
    format!(
        "{{\"id\":{},\"text\":{},\"metadata\":{},\"created_at\":{}}}",
        record.id, text, metadata, record.created_at
    )
}

/// Execute `get` against `root`: validate the id, then point-fetch.
pub(crate) fn get(root: &Path, id_raw: &str) -> Result<DispatchOutput, LibraryError> {
    // Positive-integer rule shared with the mutation verbs; malformed ids
    // are usage errors reported before any storage work.
    let id = crate::remove::parse_record_id(id_raw).map_err(|_| LibraryError::InvalidInput {
        message: format!("get requires a positive integer <ID>, found: {id_raw}"),
    })?;
    // Read-only open: a missing index is operational and creates nothing.
    let store = match crate::vec_store::SqliteStore::open_read_only(root) {
        Ok(store) => store,
        Err(_) => return Err(operational_err()),
    };
    let record = read_one(&store, id)?;
    Ok(DispatchOutput {
        format: OutputFormat::Json,
        body: output_get_json(&record),
    })
}

/// Point-fetch one record's text, canonical metadata, and creation time.
///
/// A missing row is the distinct [`LibraryError::MissingDefinition`]; every
/// other failure is the redacted operational error.
fn read_one(store: &crate::vec_store::SqliteStore, id: i64) -> Result<FetchedRecord, LibraryError> {
    let (metadata, text) = match store.read_record(id) {
        Ok(pair) => pair,
        Err(LibraryError::MissingDefinition { .. }) => {
            return Err(LibraryError::MissingDefinition { id: id.to_string() });
        }
        Err(_) => return Err(operational_err()),
    };
    let created_at = store.created_at(id).map_err(|_| operational_err())?;
    Ok(FetchedRecord {
        id,
        text,
        metadata,
        created_at,
    })
}

/// Successful `list` page: snapshot-consistent total plus ascending-id rows.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ListedPage {
    pub(crate) total: usize,
    pub(crate) records: Vec<FetchedRecord>,
}

/// Render the exact compact `list` JSON:
/// `{"total":N,"records":[{...},...]}` with rows in ascending id order.
fn output_list_json(page: &ListedPage) -> String {
    let mut out = format!("{{\"total\":{},\"records\":[", page.total);
    for (i, record) in page.records.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&output_get_json(record));
    }
    out.push_str("]}");
    out
}

/// Execute `list` against `root`: read-only snapshot scan in ascending id
/// order with `total` equal to the full filtered count from the same
/// snapshot as `records`.
///
/// `filter` is the canonical flat map (empty means no filtering): a row
/// matches only when its stored metadata contains every filter pair with
/// exact string equality. `limit`/`offset` page the filtered set
/// (`limit` arrives clamped to max 500 with default 50, `offset` defaults
/// to 0); clamping is re-applied defensively so direct callers cannot
/// exceed the contract.
pub(crate) fn list(
    root: &Path,
    filter: &BTreeMap<String, String>,
    limit: usize,
    offset: usize,
) -> Result<DispatchOutput, LibraryError> {
    // Read-only open: a missing index is operational and creates nothing.
    let store = match crate::vec_store::SqliteStore::open_read_only(root) {
        Ok(store) => store,
        Err(_) => return Err(operational_err()),
    };
    let rows = store.list_records().map_err(|_| operational_err())?;
    let mut records = Vec::new();
    for (id, text, canonical, created_at) in rows {
        let metadata = parse_canonical(&canonical)?;
        // Flat AND-filter: every filter pair must match exactly.
        if filter.iter().any(|(k, v)| metadata.get(k) != Some(v)) {
            continue;
        }
        records.push(FetchedRecord {
            id,
            text,
            metadata,
            created_at,
        });
    }
    // Snapshot-consistent total: full filtered count before paging. Rows
    // already arrive in ascending id order from the single snapshot scan.
    let total = records.len();
    let limit = limit.min(500);
    let page_records: Vec<FetchedRecord> = records.into_iter().skip(offset).take(limit).collect();
    let page = ListedPage {
        total,
        records: page_records,
    };
    Ok(DispatchOutput {
        format: OutputFormat::Json,
        body: output_list_json(&page),
    })
}

/// Parse a stored canonical metadata blob back into byte-ordered map form.
///
/// A malformed blob is an internal error mapped to the redacted operational
/// error by the caller.
fn parse_canonical(canonical: &str) -> Result<BTreeMap<String, String>, LibraryError> {
    let value: serde_json::Value =
        serde_json::from_str(canonical).map_err(|_| operational_err())?;
    let obj = value.as_object().ok_or_else(operational_err)?;
    let mut map = BTreeMap::new();
    for (k, v) in obj {
        let vs = v.as_str().ok_or_else(operational_err)?;
        map.insert(k.clone(), vs.to_string());
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_json_shape_is_exact() {
        let record = FetchedRecord {
            id: 3,
            text: "hello".to_string(),
            metadata: [
                ("b".to_string(), "2".to_string()),
                ("a".to_string(), "1".to_string()),
            ]
            .into_iter()
            .collect(),
            created_at: 1700000000000,
        };
        assert_eq!(
            output_get_json(&record),
            r#"{"id":3,"text":"hello","metadata":{"a":"1","b":"2"},"created_at":1700000000000}"#
        );
        // Text bytes are preserved exactly.
        let spaced = FetchedRecord {
            id: 1,
            text: "  hello world  ".to_string(),
            metadata: BTreeMap::new(),
            created_at: 0,
        };
        assert_eq!(
            output_get_json(&spaced),
            r#"{"id":1,"text":"  hello world  ","metadata":{},"created_at":0}"#
        );
    }

    #[test]
    fn list_json_shape_is_exact() {
        let page = ListedPage {
            total: 2,
            records: vec![
                FetchedRecord {
                    id: 1,
                    text: "a".to_string(),
                    metadata: BTreeMap::new(),
                    created_at: 10,
                },
                FetchedRecord {
                    id: 2,
                    text: "b".to_string(),
                    metadata: BTreeMap::new(),
                    created_at: 20,
                },
            ],
        };
        assert_eq!(
            output_list_json(&page),
            r#"{"total":2,"records":[{"id":1,"text":"a","metadata":{},"created_at":10},{"id":2,"text":"b","metadata":{},"created_at":20}]}"#
        );
        let empty = ListedPage {
            total: 0,
            records: Vec::new(),
        };
        assert_eq!(output_list_json(&empty), r#"{"total":0,"records":[]}"#);
    }

    #[test]
    fn get_validates_id_before_storage() {
        // Usage rejection happens before any storage work: no `.infraware`
        // artifact appears even though the root is fresh.
        let root = std::env::temp_dir().join(format!(
            "infraware-records-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let err = get(&root, "abc").unwrap_err();
        assert!(err.is_usage());
        assert!(!root.join(".infraware").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn get_and_list_on_missing_store_are_operational_and_create_nothing() {
        let root = std::env::temp_dir().join(format!(
            "infraware-records-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        for err in [
            get(&root, "1").unwrap_err(),
            list(&root, &BTreeMap::new(), 50, 0).unwrap_err(),
        ] {
            assert!(!err.is_usage());
            assert_eq!(err, operational_err());
        }
        assert!(
            !root.join(".infraware").exists(),
            "reads must not create storage"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn get_and_list_round_trip_seeded_index() {
        use crate::ingest::{Embedder, HashStore, INGEST_DIMENSION};
        struct ZeroEmbed;
        impl Embedder for ZeroEmbed {
            fn embed(&self, _text: &str) -> Result<Vec<f32>, LibraryError> {
                Ok(vec![0.0; INGEST_DIMENSION as usize])
            }
        }
        let root = std::env::temp_dir().join(format!(
            "infraware-records-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut store = crate::vec_store::SqliteStore::open(&root).unwrap();
        let canonical = crate::metadata::canonicalize_metadata(Some(r#"{"a":"1"}"#)).unwrap();
        let hash = crate::metadata::compute_identity(&canonical, "hello");
        let embedder = ZeroEmbed;
        let embedding = embedder.embed("hello").unwrap();
        let id = store
            .insert(&hash, "hello", &canonical, &embedding)
            .unwrap();
        drop(store);
        let out = get(&root, &id.to_string()).expect("get succeeds");
        let v: serde_json::Value = serde_json::from_str(&out.body).unwrap();
        assert_eq!(v["id"], id);
        assert_eq!(v["text"], serde_json::json!("hello"));
        assert_eq!(v["metadata"], serde_json::json!({"a": "1"}));
        assert!(v["created_at"].is_number());
        assert_eq!(v.as_object().unwrap().len(), 4);
        // Missing id is the distinct record-not-found error, not operational.
        assert_eq!(
            get(&root, "999").unwrap_err(),
            LibraryError::MissingDefinition {
                id: "999".to_string()
            }
        );
        let out = list(&root, &BTreeMap::new(), 50, 0).expect("list succeeds");
        let v: serde_json::Value = serde_json::from_str(&out.body).unwrap();
        assert_eq!(v["total"], 1);
        assert_eq!(v["records"].as_array().unwrap().len(), 1);
        assert_eq!(v["records"][0]["id"], id);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn get_preserves_canonical_metadata_bytes_exact() {
        // Task 1.4: reordered/whitespace input canonicalizes once at the
        // ingest boundary; the stored canonical bytes must survive the
        // write/read round trip verbatim. Assert on the raw body substring
        // (not parsed JSON) so key order and compact form are covered.
        use crate::ingest::{Embedder, HashStore, INGEST_DIMENSION};
        struct ZeroEmbed;
        impl Embedder for ZeroEmbed {
            fn embed(&self, _text: &str) -> Result<Vec<f32>, LibraryError> {
                Ok(vec![0.0; INGEST_DIMENSION as usize])
            }
        }
        let root = std::env::temp_dir().join(format!(
            "infraware-records-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut store = crate::vec_store::SqliteStore::open(&root).unwrap();
        let canonical =
            crate::metadata::canonicalize_metadata(Some(r#"{ "b" : "2" , "a" : "1" }"#)).unwrap();
        assert_eq!(canonical, r#"{"a":"1","b":"2"}"#);
        let hash = crate::metadata::compute_identity(&canonical, "hello");
        let embedding = ZeroEmbed.embed("hello").unwrap();
        let id = store
            .insert(&hash, "hello", &canonical, &embedding)
            .unwrap();
        drop(store);
        let out = get(&root, &id.to_string()).expect("get succeeds");
        assert!(
            out.body.contains(r#""metadata":{"a":"1","b":"2"}"#),
            "canonical bytes must survive verbatim: {}",
            out.body
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
