//! Thin SQLite cores for the `get` and `list` verbs.
//!
//! Reached exclusively through [`dispatch`](crate::dispatch::dispatch). Both
//! verbs are read-only: they open the index through the approved read-only
//! snapshot constructor (never creating or repairing storage) and reuse the
//! existing [`SqliteStore`](crate::vec_store::SqliteStore) read APIs.
//!
//! - `get <ID>` is a direct point fetch by positive integer record id; a
//!   missing record surfaces the distinct record-not-found error.
//! - `list` returns rules ordered by ascending id with a snapshot-consistent
//!   `total`, plus `limit` (default 50, clamped to max 500) and `offset` paging.
//!
//! Storage failures (missing, unreadable, or incompatible index) map to a
//! single detail-free operational error so the adapter reports status 1 with
//! empty stdout and one `error:` line on stderr.

use serde::Serialize;
use std::path::Path;

use crate::command::Entity;
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
    pub(crate) entity: Entity,
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
pub(crate) fn get(
    root: &Path,
    id_raw: &str,
    entity: Entity,
) -> Result<DispatchOutput, LibraryError> {
    // Positive-integer rule shared with the mutation verbs; malformed ids
    // are usage errors reported before any storage work.
    let id = crate::remove::parse_record_id(id_raw).map_err(|_| LibraryError::InvalidInput {
        message: format!("get requires a positive integer <ID>, found: {id_raw}"),
    })?;
    // Read-only open never creates storage.
    let store = match crate::vec_store::SqliteStore::open_read_only(root) {
        Ok(store) => store,
        Err(LibraryError::UninitializedStorage) => return Err(LibraryError::UninitializedStorage),
        Err(_) => return Err(operational_err()),
    };
    let record = read_one(&store, id, entity)?;
    Ok(DispatchOutput {
        format: OutputFormat::Json,
        body: output_get_json(&record),
    })
}

/// Point-fetch one record's text and creation time.
///
/// A missing row is the distinct [`LibraryError::MissingDefinition`]; every
/// other failure is the redacted operational error.
fn read_one(
    store: &crate::vec_store::SqliteStore,
    id: i64,
    entity: Entity,
) -> Result<FetchedRecord, LibraryError> {
    let (text, created_at) = store.read_entity(entity, id).map_err(|e| match e {
        LibraryError::MissingDefinition { .. } => {
            LibraryError::MissingDefinition { id: id.to_string() }
        }
        _ => operational_err(),
    })?;
    Ok(FetchedRecord {
        id,
        text,
        created_at,
        entity,
    })
}

/// Typed grouped list response shared by the JSON and YAML rendering paths.
#[derive(Debug, Serialize)]
pub(crate) struct ListResponse {
    pub(crate) limit: usize,
    pub(crate) offset: usize,
    pub(crate) rules: Vec<ListRule>,
    pub(crate) total: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct ListRule {
    pub(crate) text: String,
    pub(crate) id: i64,
    pub(crate) created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) updated_at: Option<i64>,
    pub(crate) tasks: Vec<ListTask>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ListTask {
    pub(crate) text: String,
    pub(crate) id: i64,
    pub(crate) created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) updated_at: Option<i64>,
}

/// Execute `list` against `root`: read-only snapshot scan of rules in
/// timestamp and ID order with `total` equal to the full count from the same
/// snapshot as the grouped rule page.
///
/// `limit`/`offset` page the rule set (`limit` arrives clamped to max 500
/// with default 50, `offset` defaults to 0); clamping is re-applied
/// defensively so direct callers cannot exceed the contract.
pub(crate) fn list(
    root: &Path,
    limit: usize,
    offset: usize,
) -> Result<DispatchOutput, LibraryError> {
    // Read-only open never creates storage.
    let store = match crate::vec_store::SqliteStore::open_read_only(root) {
        Ok(store) => store,
        Err(LibraryError::UninitializedStorage) => return Err(LibraryError::UninitializedStorage),
        Err(_) => return Err(operational_err()),
    };
    let mut store = store;
    let grouped = store
        .grouped_rule_page(limit, offset)
        .map_err(|_| operational_err())?;
    let response = ListResponse {
        limit: grouped.limit,
        offset: grouped.offset,
        total: grouped.total,
        rules: grouped
            .rules
            .into_iter()
            .map(|rule| ListRule {
                text: rule.text,
                id: rule.id,
                created_at: rule.created_at,
                updated_at: rule.updated_at,
                tasks: rule
                    .tasks
                    .into_iter()
                    .map(|task| ListTask {
                        text: task.text,
                        id: task.id,
                        created_at: task.created_at,
                        updated_at: task.updated_at,
                    })
                    .collect(),
            })
            .collect(),
    };
    Ok(DispatchOutput {
        format: OutputFormat::Json,
        body: serde_json::to_string(&response).expect("list response serialization must succeed"),
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
            entity: Entity::Rules,
        });
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 3);
        assert_eq!(value["text"], "canonical");
        assert!(value.get("metadata").is_none());
    }

    #[test]
    fn grouped_list_response_contains_rules_tasks_and_timestamps() {
        let response = ListResponse {
            total: 3,
            limit: 2,
            offset: 1,
            rules: vec![
                ListRule {
                    text: "rule".into(),
                    id: 2,
                    created_at: 10,
                    updated_at: None,
                    tasks: vec![],
                },
                ListRule {
                    text: "second rule".into(),
                    id: 3,
                    created_at: 11,
                    updated_at: Some(12),
                    tasks: vec![ListTask {
                        text: "task".into(),
                        id: 7,
                        created_at: 9,
                        updated_at: None,
                    }],
                },
            ],
        };
        let value = serde_json::to_value(response).unwrap();
        let keys: std::collections::BTreeSet<String> =
            value.as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            ["limit", "offset", "rules", "total"]
                .into_iter()
                .map(String::from)
                .collect()
        );
        assert_eq!(value["rules"][0]["tasks"], serde_json::json!([]));
        assert!(value["rules"][0].get("updated_at").is_none());
        assert_eq!(value["rules"][1]["tasks"][0]["id"], 7);
        assert_eq!(value["rules"][1]["updated_at"], 12);
        assert!(value["rules"][1]["tasks"][0].get("updated_at").is_none());
        let encoded = value.to_string();
        assert!(!encoded.contains("records"));
        assert!(!encoded.contains("entity"));
        assert!(!encoded.contains("type"));
    }

    #[test]
    fn invalid_id_is_usage_before_storage() {
        let root =
            std::env::temp_dir().join(format!("infraware-records-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert!(
            get(&root, "not-an-id", Entity::Rules)
                .unwrap_err()
                .is_usage()
        );
        assert!(!root.join(".infraware").exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
