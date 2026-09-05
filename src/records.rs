//! Thin SQLite cores for the `get` and `list` verbs.
//!
//! Reached exclusively through [`dispatch`](crate::dispatch::dispatch). Both
//! verbs are read-only: they open the index through the approved read-only
//! snapshot constructor (never creating or repairing storage) and reuse the
//! existing [`SqliteStore`](crate::vec_store::SqliteStore) read APIs.
//!
//! - `get <ID>` is a direct point fetch by positive integer record id; a
//!   missing record surfaces the distinct record-not-found error.
//! - `list` returns records ordered by ascending id with a snapshot-consistent
//!   `total`, plus `limit` (default 50, clamped to max 500) and `offset` paging.
//!
//! Storage failures (missing, unreadable, or incompatible index) map to a
//! single detail-free operational error so the adapter reports status 1 with
//! empty stdout and one `error:` line on stderr.

use std::path::Path;

use crate::dispatch::DispatchOutput;
use crate::error::LibraryError;
use crate::format::OutputFormat;

/// Generic operational failure for the get/list read paths.
///
/// Detail-free like the rule/ingest/remove gates: storage paths, SQL, and
/// record contents never reach diagnostics. Non-usage, so the adapter
/// reports status 1 with empty stdout. The distinct record-not-found error
/// passes through unchanged.
fn operational_err() -> LibraryError {
    LibraryError::Io {
        message: "store unavailable".to_string(),
    }
}

/// Successful `get` record: stable id, full text bytes, and creation timestamp.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FetchedRecord {
    pub(crate) id: i64,
    pub(crate) text: String,
    pub(crate) created_at: i64,
}

/// Render the exact compact text-only `get` JSON:
/// `{"id":N,"text":...,"created_at":N}`.
///
/// Field order is fixed; text is preserved byte-exactly.
fn output_get_json(record: &FetchedRecord) -> String {
    let text = serde_json::to_string(&record.text).expect("record text serialization must succeed");
    format!(
        "{{\"id\":{},\"text\":{},\"created_at\":{}}}",
        record.id, text, record.created_at
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

/// Point-fetch one record's text and creation time.
///
/// A missing row is the distinct [`LibraryError::MissingDefinition`]; every
/// other failure is the redacted operational error.
fn read_one(store: &crate::vec_store::SqliteStore, id: i64) -> Result<FetchedRecord, LibraryError> {
    let text = match store.read_record(id) {
        Ok(text) => text,
        Err(LibraryError::MissingDefinition { .. }) => {
            return Err(LibraryError::MissingDefinition { id: id.to_string() });
        }
        Err(_) => return Err(operational_err()),
    };
    let created_at = store.created_at(id).map_err(|_| operational_err())?;
    Ok(FetchedRecord {
        id,
        text,
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
    for (id, text, created_at) in rows {
        records.push(FetchedRecord {
            id,
            text,
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn text_only_get_output_omits_metadata() {
        let output = output_get_json(&FetchedRecord {
            id: 7,
            text: "canonical".into(),
            created_at: 42,
        });
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 3);
        assert_eq!(value["text"], "canonical");
        assert!(value.get("metadata").is_none());
    }

    #[test]
    fn list_pagination_preserves_total_and_order() {
        let page = ListedPage {
            total: 3,
            records: vec![
                FetchedRecord {
                    id: 2,
                    text: "b".into(),
                    created_at: 2,
                },
                FetchedRecord {
                    id: 3,
                    text: "c".into(),
                    created_at: 3,
                },
            ],
        };
        let value: serde_json::Value = serde_json::from_str(&output_list_json(&page)).unwrap();
        assert_eq!(value["total"], 3);
        assert_eq!(value["records"][0]["id"], 2);
        assert_eq!(value["records"][1]["id"], 3);
    }

    #[test]
    fn invalid_id_is_usage_before_storage() {
        let root =
            std::env::temp_dir().join(format!("infraware-records-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert!(get(&root, "not-an-id").unwrap_err().is_usage());
        assert!(!root.join(".infraware").exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
