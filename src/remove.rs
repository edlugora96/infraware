//! semantic search `remove` execution seam.
//!
//! Grammar: `remove <positive-integer-id>`. The ID is validated to a
//! positive integer before any storage work (zero, negative, non-integer,
//! and missing values are usage errors with status 2). The index is opened
//! read-write without creation: missing storage is an operational error
//! with status 1 and creates nothing. Deletion runs atomically through
//! [`SqliteStore::remove_record`](crate::vec_store::SqliteStore::remove_record)
//! (evidence-backed `DELETE FROM vecs WHERE rowid = ?` plus side-table and
//! record deletes in one transaction); a missing record surfaces the
//! distinct record-not-found error, never the generic operational error.
//! No model work runs on this path. No VACUUM.

use std::path::Path;

use crate::dispatch::DispatchOutput;
use crate::error::LibraryError;
use crate::format::OutputFormat;

/// Generic operational failure for the remove path.
///
/// Detail-free like the rule/ingest gates: storage paths, SQL, and
/// vectors never reach diagnostics. Non-usage, so the adapter reports
/// status 1 with empty stdout. Record-not-found keeps its distinct error.
fn operational_err() -> LibraryError {
    LibraryError::Io {
        message: "remove failed".to_string(),
    }
}

/// Parse a public semantic search record ID: strictly ASCII digits, value >= 1, fits `i64`.
///
/// Leading zeros are accepted (`"007"` is record 7). Empty, non-digit,
/// zero, negative, and overflowing inputs are usage errors.
pub(crate) fn parse_record_id(raw: &str) -> Result<i64, LibraryError> {
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(LibraryError::InvalidInput {
            message: format!("remove requires a positive integer <ID>, found: {raw}"),
        });
    }
    match raw.parse::<i64>() {
        Ok(id) if id >= 1 => Ok(id),
        _ => Err(LibraryError::InvalidInput {
            message: format!("remove requires a positive integer <ID>, found: {raw}"),
        }),
    }
}

/// Render the exact compact success JSON: `{"id":N,"removed":true}`.
fn output_json(id: i64) -> String {
    format!("{{\"id\":{id},\"removed\":true}}")
}

/// Execute `remove` against `root`.
pub(crate) fn run(root: &Path, id_raw: &str) -> Result<DispatchOutput, LibraryError> {
    // Validate the ID before opening storage or touching the index.
    let id = parse_record_id(id_raw)?;
    // Never creates storage: missing index is the generic operational error.
    let mut store = match crate::vec_store::SqliteStore::open_existing_rw(root) {
        Ok(store) => store,
        Err(LibraryError::UninitializedStorage) => return Err(LibraryError::UninitializedStorage),
        Err(_) => return Err(operational_err()),
    };
    match store.remove_record(id) {
        Ok(()) => Ok(DispatchOutput {
            format: OutputFormat::Json,
            body: output_json(id),
        }),
        // Distinct record-not-found propagates unchanged (status 1, not the
        // generic storage error); every other failure is redacted.
        Err(LibraryError::MissingDefinition { .. }) => {
            Err(LibraryError::MissingDefinition { id: id.to_string() })
        }
        Err(_) => Err(operational_err()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::{HashStore, INGEST_DIMENSION};

    fn tmp_root() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "infraware-remove-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn seed_one(root: &Path) -> i64 {
        let mut store = crate::vec_store::SqliteStore::open(root).unwrap();
        let emb = vec![0.5f32; INGEST_DIMENSION as usize];
        store.insert("rm-seed", "hello", &emb).unwrap()
    }

    #[test]
    fn parse_record_id_accepts_positive_integers() {
        assert_eq!(parse_record_id("1").unwrap(), 1);
        assert_eq!(parse_record_id("42").unwrap(), 42);
        assert_eq!(parse_record_id("007").unwrap(), 7);
    }

    #[test]
    fn parse_record_id_rejects_non_positive_shapes() {
        for bad in ["", "0", "-1", "-5", "abc", "1.5", " 1", "1 ", "+5", "0x10"] {
            assert!(
                parse_record_id(bad).is_err_and(|e| e.is_usage()),
                "{bad:?} must be a usage error"
            );
        }
        // Overflowing i64 is a usage error, not a panic.
        assert!(parse_record_id("99999999999999999999999").is_err());
    }

    #[test]
    fn remove_success_emits_exact_json() {
        let root = tmp_root();
        let id = seed_one(&root);
        let out = run(&root, &id.to_string()).expect("remove succeeds");
        assert_eq!(out.body, format!("{{\"id\":{id},\"removed\":true}}"));
        // Repeating removal is record-not-found, not generic failure.
        let err = run(&root, &id.to_string()).unwrap_err();
        assert_eq!(err, LibraryError::MissingDefinition { id: id.to_string() });
        assert!(!err.is_usage());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn remove_missing_storage_is_typed_and_creates_nothing() {
        let root = tmp_root();
        let err = run(&root, "1").unwrap_err();
        assert_eq!(err, LibraryError::UninitializedStorage);
        assert!(
            !root.join(".infraware").exists(),
            "remove must not create storage"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn remove_validates_id_before_storage() {
        // Usage rejection happens before any storage work: no `.infraware`
        // artifact appears even though the root is fresh.
        let root = tmp_root();
        let err = run(&root, "abc").unwrap_err();
        assert!(err.is_usage());
        assert!(!root.join(".infraware").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
