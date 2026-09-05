//! Atomic ingest write path. This is the only fact creator.
//!
//! One call ingests exactly one fact/query pair, preserving both text values
//! exactly and writing them atomically. The whole operation fails if any step
//! fails; argument shape is enforced by the parser before this seam.

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
    pub fact_id: i64,
    pub query_id: i64,
    pub created_at: i64,
    pub duplicate: bool,
}

#[allow(dead_code, unreachable_pub)]
pub fn output_json(out: &IngestOutput) -> String {
    format!(
        "{{\"fact_id\":{},\"query_id\":{},\"created_at\":{},\"duplicate\":{}}}",
        out.fact_id, out.query_id, out.created_at, out.duplicate
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
    _metadata_raw: Option<&str>,
) -> Result<IngestOutput, LibraryError> {
    if text.trim().is_empty() {
        return Err(LibraryError::InvalidInput {
            message: "ingest text must not be empty".into(),
        });
    }
    let canonical = "{}";
    let hash = "";
    if let Some(id) = store.find_by_hash(hash)? {
        return Ok(IngestOutput {
            fact_id: id,
            query_id: 0,
            created_at: 0,
            duplicate: true,
        });
    }
    let embedding = embedder.embed(text)?;
    let id = store.insert(hash, text, canonical, &embedding)?;
    Ok(IngestOutput {
        fact_id: id,
        query_id: 0,
        created_at: 0,
        duplicate: false,
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

/// Production ingest validates text, embeds both inputs, and writes one pair.
pub(crate) fn run(
    root: &std::path::Path,
    fact_text: &str,
    query_text: &str,
) -> Result<DispatchOutput, LibraryError> {
    if fact_text.trim().is_empty() || query_text.trim().is_empty() {
        return Err(LibraryError::InvalidInput {
            message: "ingest text must not be empty".into(),
        });
    }
    let paths = crate::model_cache::ensure_model_files().map_err(|_| operational_err())?;
    let embedder = crate::embed::BgeEmbedder::load(&paths).map_err(|_| operational_err())?;
    let fv = embedder.embed(fact_text).map_err(|_| operational_err())?;
    let qv = embedder.embed(query_text).map_err(|_| operational_err())?;
    let mut store = crate::vec_store::SqliteStore::open(root)?;
    let (fact_id, query_id, created_at, duplicate) =
        store.insert_pair(fact_text, query_text, &fv, &qv)?;
    let out = IngestOutput {
        fact_id,
        query_id,
        created_at,
        duplicate,
    };
    Ok(DispatchOutput {
        format: OutputFormat::Json,
        body: output_json(&out),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipt_json_has_exact_v2_fields() {
        let out = IngestOutput {
            fact_id: 7,
            query_id: 8,
            created_at: 123,
            duplicate: false,
        };
        assert_eq!(
            output_json(&out),
            r#"{"fact_id":7,"query_id":8,"created_at":123,"duplicate":false}"#
        );
    }
    #[test]
    fn duplicate_receipt_is_distinct() {
        let out = IngestOutput {
            fact_id: 7,
            query_id: 8,
            created_at: 123,
            duplicate: true,
        };
        let value: serde_json::Value = serde_json::from_str(&output_json(&out)).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 4);
        assert_eq!(value["duplicate"], true);
    }

    #[test]
    fn paired_store_writes_independent_texts_vectors_relation_and_timestamp() {
        let root = std::env::temp_dir().join(format!("infraware-pair-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut store = crate::vec_store::SqliteStore::open(&root).unwrap();
        let fact = vec![1.0f32; INGEST_DIMENSION as usize];
        let query = vec![0.0f32; INGEST_DIMENSION as usize];
        let (fact_id, query_id, created_at, duplicate) = store
            .insert_pair("fact exact", "query exact", &fact, &query)
            .unwrap();
        assert!(!duplicate);
        assert!(fact_id > 0 && query_id > 0 && created_at > 0);
        drop(store);
        let conn = rusqlite::Connection::open(root.join(".infraware/vec-index.sqlite")).unwrap();
        let row: (String, String, i64) = conn.query_row(
            "SELECT f.text,q.text,p.created_at FROM facts f JOIN pair_relations p ON p.fact_id=f.fact_id JOIN queries q ON q.query_id=p.query_id WHERE f.fact_id=?1",
            [fact_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
        assert_eq!(row.0, "fact exact");
        assert_eq!(row.1, "query exact");
        assert_eq!(row.2, created_at);
        let schema: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        for table in [
            "facts",
            "queries",
            "fact_vecs",
            "query_vecs",
            "pair_relations",
            "query_runs",
            "query_fact_links",
            "model_meta",
        ] {
            assert!(schema.iter().any(|name| name == table), "missing {table}");
        }
        let fact_columns: Vec<String> = conn
            .prepare("PRAGMA table_info(facts)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(fact_columns, ["fact_id", "text", "created_at"]);
        let relation: (i64, i64, i64) = conn
            .query_row(
                "SELECT fact_id,query_id,created_at FROM pair_relations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(relation, (fact_id, query_id, created_at));
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM fact_vecs", [], |r| r.get(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT COUNT(*) FROM query_vecs", [], |r| r.get(0))
                .unwrap(),
            1
        );
        assert!(
            conn.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM facts WHERE text LIKE '%metadata%'",
                [],
                |r| r.get(0)
            )
            .unwrap()
                == 0
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn duplicate_relation_reuses_ids_and_original_timestamp() {
        let root = std::env::temp_dir().join(format!("infraware-pair-dup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut store = crate::vec_store::SqliteStore::open(&root).unwrap();
        let vector = vec![1.0f32; INGEST_DIMENSION as usize];
        let first = store
            .insert_pair("canonical fact", "canonical query", &vector, &vector)
            .unwrap();
        let second = store
            .insert_pair("different spelling", "different question", &vector, &vector)
            .unwrap();
        assert_eq!((second.0, second.1, second.2), (first.0, first.1, first.2));
        assert!(!first.3);
        assert!(second.3);
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn receipt_contains_only_four_fields() {
        let receipt = output_json(&IngestOutput {
            fact_id: 3,
            query_id: 4,
            created_at: 5,
            duplicate: false,
        });
        let value: serde_json::Value = serde_json::from_str(&receipt).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 4);
        assert_eq!(value["fact_id"], 3);
        assert_eq!(value["query_id"], 4);
        assert_eq!(value["created_at"], 5);
        assert_eq!(value["duplicate"], false);
    }
}
