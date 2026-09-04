//! Lazy sqlite-vec backed [`crate::ingest::HashStore`].

use std::path::{Path, PathBuf};

use crate::error::LibraryError;
use crate::ingest::{HashStore, INGEST_DIMENSION, INGEST_MODEL, INGEST_MODEL_REVISION};

fn io_err(msg: impl std::fmt::Display) -> LibraryError {
    LibraryError::Io {
        message: msg.to_string(),
    }
}

fn vec_path(root: &Path) -> PathBuf {
    root.join(".infraware").join("vec-index.sqlite")
}

pub(crate) struct SqliteStore {
    conn: rusqlite::Connection,
}

impl SqliteStore {
    pub(crate) fn open(root: &Path) -> Result<Self, LibraryError> {
        let dir = root.join(".infraware");
        std::fs::create_dir_all(&dir).map_err(io_err)?;
        crate::vec_shim::ensure_registered();
        let conn = rusqlite::Connection::open(vec_path(root)).map_err(io_err)?;
        let this = Self { conn };
        this.init_schema()?;
        Ok(this)
    }

    fn init_schema(&self) -> Result<(), LibraryError> {
        self.conn
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=FULL;
                 CREATE TABLE IF NOT EXISTS records(
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   hash TEXT NOT NULL UNIQUE,
                   text TEXT NOT NULL,
                   metadata TEXT NOT NULL,
                   created_ms INTEGER NOT NULL DEFAULT (strftime('%s','now')*1000)
                 );
                 CREATE TABLE IF NOT EXISTS meta_kv(
                   record_id INTEGER NOT NULL REFERENCES records(id),
                   k TEXT NOT NULL,
                   v TEXT NOT NULL,
                   PRIMARY KEY(record_id,k)
                 );
                 CREATE INDEX IF NOT EXISTS idx_meta_kv_kv ON meta_kv(k,v);
                 CREATE VIRTUAL TABLE IF NOT EXISTS vecs USING vec0(embedding float[384] distance_metric=cosine);
                 CREATE TABLE IF NOT EXISTS model_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);",
            )
            .map_err(io_err)?;
        for (k, v) in [
            ("schema_version", "1"),
            ("model", INGEST_MODEL),
            ("revision", INGEST_MODEL_REVISION),
            ("dimension", "384"),
            ("metric", "cosine"),
            ("normalized", "1"),
        ] {
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO model_meta(key,value) VALUES(?1,?2)",
                    rusqlite::params![k, v],
                )
                .map_err(io_err)?;
        }
        Ok(())
    }
}

impl HashStore for SqliteStore {
    fn find_by_hash(&self, hash: &str) -> Result<Option<i64>, LibraryError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM records WHERE hash=?1")
            .map_err(io_err)?;
        let mut rows = stmt.query(rusqlite::params![hash]).map_err(io_err)?;
        match rows.next().map_err(io_err)? {
            Some(r) => Ok(Some(r.get(0).map_err(io_err)?)),
            None => Ok(None),
        }
    }

    fn insert(
        &mut self,
        hash: &str,
        text: &str,
        canonical: &str,
        embedding: &[f32],
    ) -> Result<i64, LibraryError> {
        if embedding.len() != INGEST_DIMENSION as usize {
            return Err(io_err(format!(
                "invalid embedding length {}",
                embedding.len()
            )));
        }
        // One atomic write transaction.
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(io_err)?;
        let record_id: i64 = match tx.execute(
            "INSERT INTO records(hash,text,metadata) VALUES(?1,?2,?3)",
            rusqlite::params![hash, text, canonical],
        ) {
            Ok(_) => tx.last_insert_rowid(),
            Err(e) if e.to_string().contains("UNIQUE") => {
                drop(tx);
                // UNIQUE race: fall back to the winning row.
                return self.find_by_hash(hash)?.ok_or_else(|| io_err(e));
            }
            Err(e) => return Err(io_err(e)),
        };
        // Any failure before commit rolls back (tx drop) leaving no partial rows.
        let step: Result<(), LibraryError> = (|| {
            let meta: serde_json::Value = serde_json::from_str(canonical).map_err(io_err)?;
            let obj = meta
                .as_object()
                .ok_or_else(|| io_err("metadata not object"))?;
            for (k, v) in obj {
                let vs = v.as_str().ok_or_else(|| io_err("metadata not flat"))?;
                tx.execute(
                    "INSERT INTO meta_kv(record_id,k,v) VALUES(?1,?2,?3)",
                    rusqlite::params![record_id, k, vs],
                )
                .map_err(io_err)?;
            }
            let mut bytes = Vec::with_capacity(embedding.len() * 4);
            for f in embedding {
                bytes.extend_from_slice(&f.to_le_bytes());
            }
            tx.execute(
                "INSERT INTO vecs(rowid, embedding) VALUES(?1, ?2)",
                rusqlite::params![record_id, bytes],
            )
            .map_err(io_err)?;
            Ok(())
        })();
        if let Err(e) = step {
            drop(tx); // rollback
            return Err(e);
        }
        tx.commit().map_err(io_err)?;
        Ok(record_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::INGEST_DIMENSION;

    fn tmp_root() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "infraware-vec-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn count(conn: &rusqlite::Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn lazy_path_and_ddl() {
        let root = tmp_root();
        assert!(!vec_path(&root).exists());
        let store = SqliteStore::open(&root).unwrap();
        assert!(vec_path(&root).is_file());
        let ddl: String = store
            .conn
            .query_row("SELECT sql FROM sqlite_master WHERE name='vecs'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(ddl.contains("float[384]"), "ddl was: {ddl}");
        assert!(!ddl.contains("float[3] ") && !ddl.contains("float[3]]"));
        // meta_kv + index + model metadata present
        let idx: String = store
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='idx_meta_kv_kv'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(idx.contains("meta_kv"));
        let dim: String = store
            .conn
            .query_row(
                "SELECT value FROM model_meta WHERE key='dimension'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dim, "384");
        let model: String = store
            .conn
            .query_row("SELECT value FROM model_meta WHERE key='model'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(model, INGEST_MODEL);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn insert_links_rowid_and_unique_hash() {
        let root = tmp_root();
        let mut store = SqliteStore::open(&root).unwrap();
        let emb = vec![0.5f32; INGEST_DIMENSION as usize];
        let id = store.insert("h1", "hello", r#"{"a":"1"}"#, &emb).unwrap();
        assert_eq!(store.find_by_hash("h1").unwrap(), Some(id));
        // meta_kv row + vecs rowid linkage
        let v: String = store
            .conn
            .query_row(
                "SELECT v FROM meta_kv WHERE record_id=?1 AND k='a'",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "1");
        let nvec: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM vecs WHERE rowid=?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nvec, 1);
        // duplicate hash rejected by UNIQUE
        let err = store
            .conn
            .execute(
                "INSERT INTO records(hash,text,metadata) VALUES('h1','x','{}')",
                [],
            )
            .unwrap_err();
        assert!(err.to_string().contains("UNIQUE"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn init_does_not_touch_vec_db() {
        let root = tmp_root();
        crate::lifecycle::init(&root).unwrap();
        assert!(!vec_path(&root).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn malformed_metadata_rolls_back() {
        let root = tmp_root();
        let mut store = SqliteStore::open(&root).unwrap();
        let emb = vec![0.1f32; INGEST_DIMENSION as usize];
        // Seed one good record so we can prove counts are unchanged.
        let good = store.insert("good", "t", "{}", &emb).unwrap();
        assert!(good > 0);
        let before_r = count(&store.conn, "SELECT count(*) FROM records");
        let before_m = count(&store.conn, "SELECT count(*) FROM meta_kv");
        let before_v = count(&store.conn, "SELECT count(*) FROM vecs");
        // Malformed canonical: parses as JSON? "[1,2]" is valid JSON but not an
        // object, so failure happens after the records insert -> must roll back.
        let res = store.insert("badhash", "t2", "[1,2]", &emb);
        assert!(res.is_err());
        assert_eq!(count(&store.conn, "SELECT count(*) FROM records"), before_r);
        assert_eq!(count(&store.conn, "SELECT count(*) FROM meta_kv"), before_m);
        assert_eq!(count(&store.conn, "SELECT count(*) FROM vecs"), before_v);
        assert_eq!(store.find_by_hash("badhash").unwrap(), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
