//! v2 paired storage.
use crate::{
    error::LibraryError,
    ingest::{HashStore, INGEST_MODEL, INGEST_MODEL_REVISION},
};
use rusqlite::OptionalExtension;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
fn er(x: impl std::fmt::Display) -> LibraryError {
    LibraryError::Io {
        message: x.to_string(),
    }
}
fn path(r: &Path) -> PathBuf {
    r.join(".infraware/vec-index.sqlite")
}
fn enc(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
fn dec(b: &[u8]) -> Result<Vec<f32>, LibraryError> {
    if b.len() != 1536 {
        return Err(er("invalid vector"));
    }
    Ok(b.chunks_exact(4)
        .map(|x| f32::from_le_bytes([x[0], x[1], x[2], x[3]]))
        .collect())
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|x| x.as_millis() as i64)
        .unwrap_or(0)
}
pub(crate) struct QueryCandidateRow(
    pub(crate) i64,
    pub(crate) i64,
    pub(crate) i64,
    pub(crate) Vec<f32>,
    pub(crate) String,
);

pub(crate) struct SqliteStore {
    conn: rusqlite::Connection,
}
impl SqliteStore {
    pub(crate) fn open(r: &Path) -> Result<Self, LibraryError> {
        std::fs::create_dir_all(r.join(".infraware")).map_err(er)?;
        let fresh = !path(r).exists();
        crate::vec_shim::ensure_registered();
        let mut s = Self {
            conn: rusqlite::Connection::open(path(r)).map_err(er)?,
        };
        if fresh {
            s.init()?;
        }
        s.verify_query_compatible()?;
        Ok(s)
    }
    pub(crate) fn open_existing_rw(r: &Path) -> Result<Self, LibraryError> {
        if !path(r).is_file() {
            return Err(er("index missing"));
        }
        crate::vec_shim::ensure_registered();
        let s = Self {
            conn: rusqlite::Connection::open(path(r)).map_err(er)?,
        };
        s.verify_query_compatible()?;
        Ok(s)
    }
    pub(crate) fn open_read_only(r: &Path) -> Result<Self, LibraryError> {
        if !path(r).is_file() {
            return Err(er("index missing"));
        }
        crate::vec_shim::ensure_registered();
        let s = Self {
            conn: rusqlite::Connection::open_with_flags(
                path(r),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(er)?,
        };
        s.verify_query_compatible()?;
        Ok(s)
    }
    fn init(&mut self) -> Result<(), LibraryError> {
        let tx = self.conn.transaction().map_err(er)?;
        tx.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE facts(fact_id INTEGER PRIMARY KEY AUTOINCREMENT,text TEXT NOT NULL,created_at INTEGER NOT NULL);CREATE TABLE queries(query_id INTEGER PRIMARY KEY AUTOINCREMENT,text TEXT NOT NULL,created_at INTEGER NOT NULL);CREATE VIRTUAL TABLE fact_vecs USING vec0(embedding float[384] distance_metric=cosine);CREATE VIRTUAL TABLE query_vecs USING vec0(embedding float[384] distance_metric=cosine);CREATE TABLE pair_relations(fact_id INTEGER REFERENCES facts(fact_id),query_id INTEGER REFERENCES queries(query_id),created_at INTEGER,PRIMARY KEY(fact_id,query_id));CREATE TABLE model_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);").map_err(er)?;
        for (k, v) in [
            ("schema_version", "2"),
            ("model", INGEST_MODEL),
            ("revision", INGEST_MODEL_REVISION),
            ("dimension", "384"),
            ("metric", "cosine"),
            ("normalized", "1"),
            ("encoding", "little-endian-f32"),
        ] {
            tx.execute(
                "INSERT INTO model_meta VALUES(?1,?2)",
                rusqlite::params![k, v],
            )
            .map_err(er)?;
        }
        tx.commit().map_err(er)
    }
    pub(crate) fn verify_query_compatible(&self) -> Result<(), LibraryError> {
        let expected = [
            "facts",
            "queries",
            "fact_vecs",
            "query_vecs",
            "pair_relations",
            "model_meta",
        ];
        let tables = self
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table'")
            .map_err(er)?
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(er)?
            .map(|row| row.map_err(er))
            .collect::<Result<Vec<_>, _>>()?;
        for table in &tables {
            let canonical = expected.iter().any(|name| name == table);
            let sqlite_sequence = table == "sqlite_sequence";
            let vec_shadow = ["fact_vecs", "query_vecs"].iter().any(|prefix| {
                [
                    "_chunks",
                    "_rowids",
                    "_vector_chunks00",
                    "_info",
                    "_metadata",
                    "_partition",
                ]
                .iter()
                .any(|suffix| table == &format!("{prefix}{suffix}"))
            });
            if !canonical && !sqlite_sequence && !vec_shadow {
                return Err(er("index incompatible"));
            }
        }
        if expected
            .iter()
            .any(|name| !tables.iter().any(|table| table == name))
        {
            return Err(er("index incompatible"));
        }
        for t in ["fact_vecs", "query_vecs"] {
            let s: String = self
                .conn
                .query_row("SELECT sql FROM sqlite_master WHERE name=?1", [t], |r| {
                    r.get(0)
                })
                .map_err(er)?;
            if !s.contains("float[384]") || !s.contains("cosine") {
                return Err(er("index incompatible"));
            }
        }
        for (k, v) in [
            ("schema_version", "2"),
            ("model", INGEST_MODEL),
            ("revision", INGEST_MODEL_REVISION),
            ("dimension", "384"),
            ("metric", "cosine"),
            ("normalized", "1"),
            ("encoding", "little-endian-f32"),
        ] {
            if self
                .conn
                .query_row::<String, _, _>("SELECT value FROM model_meta WHERE key=?1", [k], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(er)?
                .as_deref()
                != Some(v)
            {
                return Err(er("index incompatible"));
            }
        }
        Ok(())
    }
    pub(crate) fn insert_pair(
        &mut self,
        f: &str,
        q: &str,
        fv: &[f32],
        qv: &[f32],
    ) -> Result<(i64, i64, i64, bool), LibraryError> {
        let tx = self.conn.transaction().map_err(er)?;
        let t = now();
        let nearest = |table: &str, vector: &[f32]| -> Result<Option<i64>, LibraryError> {
            let sql = format!("SELECT rowid,embedding FROM {table}");
            let mut st = tx.prepare(&sql).map_err(er)?;
            let mut best: Option<(f64, i64)> = None;
            let rows = st
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))
                .map_err(er)?;
            for row in rows {
                let (id, bytes) = row.map_err(er)?;
                let d = crate::query::cosine_distance(vector, &dec(&bytes)?);
                if d <= 0.15 && best.as_ref().is_none_or(|b| (d, id) < (b.0, b.1)) {
                    best = Some((d, id));
                }
            }
            Ok(best.map(|(_, id)| id))
        };
        let fi = match nearest("fact_vecs", fv)? {
            Some(id) => id,
            None => {
                tx.execute(
                    "INSERT INTO facts(text,created_at)VALUES(?1,?2)",
                    rusqlite::params![f, t],
                )
                .map_err(er)?;
                let id = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO fact_vecs(rowid,embedding)VALUES(?1,?2)",
                    rusqlite::params![id, enc(fv)],
                )
                .map_err(er)?;
                id
            }
        };
        let qi = match nearest("query_vecs", qv)? {
            Some(id) => id,
            None => {
                tx.execute(
                    "INSERT INTO queries(text,created_at)VALUES(?1,?2)",
                    rusqlite::params![q, t],
                )
                .map_err(er)?;
                let id = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO query_vecs(rowid,embedding)VALUES(?1,?2)",
                    rusqlite::params![id, enc(qv)],
                )
                .map_err(er)?;
                id
            }
        };
        let existing: Option<i64> = tx
            .query_row(
                "SELECT created_at FROM pair_relations WHERE fact_id=?1 AND query_id=?2",
                rusqlite::params![fi, qi],
                |r| r.get(0),
            )
            .optional()
            .map_err(er)?;
        if let Some(created) = existing {
            tx.commit().map_err(er)?;
            return Ok((fi, qi, created, true));
        }
        tx.execute(
            "INSERT INTO pair_relations VALUES(?1,?2,?3)",
            rusqlite::params![fi, qi, t],
        )
        .map_err(er)?;
        tx.commit().map_err(er)?;
        Ok((fi, qi, t, false))
    }
    pub(crate) fn read_fact_text(&self, id: i64) -> Result<String, LibraryError> {
        self.conn
            .query_row("SELECT text FROM facts WHERE fact_id=?1", [id], |r| {
                r.get(0)
            })
            .map_err(er)
    }
    pub(crate) fn read_fact(&self, id: i64) -> Result<(String, i64), LibraryError> {
        self.conn
            .query_row(
                "SELECT text,created_at FROM facts WHERE fact_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(er)
    }
    pub(crate) fn insert_query_for_fact(
        &mut self,
        fact_id: i64,
        query_text: &str,
        query_embedding: &[f32],
    ) -> Result<(), LibraryError> {
        let tx = self.conn.transaction().map_err(er)?;
        let t = now();
        let mut st = tx
            .prepare("SELECT rowid,embedding FROM query_vecs")
            .map_err(er)?;
        let mut nearest = None;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))
            .map_err(er)?;
        for row in rows {
            let (id, bytes) = row.map_err(er)?;
            let d = crate::query::cosine_distance(query_embedding, &dec(&bytes)?);
            if d <= 0.15
                && nearest
                    .as_ref()
                    .is_none_or(|(best, best_id)| (d, id) < (*best, *best_id))
            {
                nearest = Some((d, id));
            }
        }
        drop(st);
        let query_id = if let Some((_, id)) = nearest {
            id
        } else {
            tx.execute(
                "INSERT INTO queries(text,created_at)VALUES(?1,?2)",
                rusqlite::params![query_text, t],
            )
            .map_err(er)?;
            let id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO query_vecs(rowid,embedding)VALUES(?1,?2)",
                rusqlite::params![id, enc(query_embedding)],
            )
            .map_err(er)?;
            id
        };
        tx.execute(
            "INSERT OR IGNORE INTO pair_relations VALUES(?1,?2,?3)",
            rusqlite::params![fact_id, query_id, t],
        )
        .map_err(er)?;
        tx.commit().map_err(er)
    }
    pub(crate) fn created_at(&self, id: i64) -> Result<i64, LibraryError> {
        self.conn
            .query_row("SELECT created_at FROM facts WHERE fact_id=?1", [id], |r| {
                r.get(0)
            })
            .map_err(er)
    }
    pub(crate) fn list_records(&self) -> Result<Vec<(i64, String, String, i64)>, LibraryError> {
        let mut s = self
            .conn
            .prepare("SELECT fact_id,text,'{}',created_at FROM facts ORDER BY fact_id")
            .map_err(er)?;
        s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(er)?
            .map(|x| x.map_err(er))
            .collect()
    }
    pub(crate) fn read_record(
        &self,
        id: i64,
    ) -> Result<(BTreeMap<String, String>, String), LibraryError> {
        Ok((BTreeMap::new(), self.read_fact(id)?.0))
    }
    pub(crate) fn remove_record(&mut self, id: i64) -> Result<(), LibraryError> {
        let tx = self.conn.transaction().map_err(er)?;
        tx.execute("DELETE FROM pair_relations WHERE fact_id=?1", [id])
            .map_err(er)?;
        tx.execute("DELETE FROM fact_vecs WHERE rowid=?1", [id])
            .map_err(er)?;
        if tx
            .execute("DELETE FROM facts WHERE fact_id=?1", [id])
            .map_err(er)?
            != 1
        {
            return Err(LibraryError::MissingDefinition { id: id.to_string() });
        };
        tx.commit().map_err(er)
    }
    pub(crate) fn apply_patch(
        &mut self,
        id: i64,
        text: &str,
        v: Option<&[f32]>,
    ) -> Result<(), LibraryError> {
        let tx = self.conn.transaction().map_err(er)?;
        if tx
            .execute(
                "UPDATE facts SET text=?1 WHERE fact_id=?2",
                rusqlite::params![text, id],
            )
            .map_err(er)?
            != 1
        {
            return Err(er("missing fact"));
        };
        if let Some(v) = v {
            let mut candidate = None;
            let mut st = tx
                .prepare("SELECT rowid,embedding FROM fact_vecs WHERE rowid<>?1")
                .map_err(er)?;
            let rows = st
                .query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))
                .map_err(er)?;
            for row in rows {
                let (other, bytes) = row.map_err(er)?;
                let distance = crate::query::cosine_distance(v, &dec(&bytes)?);
                if distance <= 0.15
                    && candidate
                        .as_ref()
                        .is_none_or(|(d, existing)| (distance, other) < (*d, *existing))
                {
                    candidate = Some((distance, other));
                }
            }
            drop(st);
            if let Some((_, canonical)) = candidate {
                tx.execute(
                    "UPDATE OR IGNORE pair_relations SET fact_id=?1 WHERE fact_id=?2",
                    rusqlite::params![canonical, id],
                )
                .map_err(er)?;
                tx.execute("DELETE FROM pair_relations WHERE fact_id=?1", [id])
                    .map_err(er)?;
                tx.execute("DELETE FROM fact_vecs WHERE rowid=?1", [id])
                    .map_err(er)?;
                tx.execute("DELETE FROM facts WHERE fact_id=?1", [id])
                    .map_err(er)?;
            } else {
                tx.execute("DELETE FROM fact_vecs WHERE rowid=?1", [id])
                    .map_err(er)?;
                tx.execute(
                    "INSERT INTO fact_vecs(rowid,embedding)VALUES(?1,?2)",
                    rusqlite::params![id, enc(v)],
                )
                .map_err(er)?;
            }
        }
        tx.commit().map_err(er)
    }
    #[allow(dead_code)]
    pub(crate) fn scan_filtered_embeddings(
        &self,
        _: &BTreeMap<String, String>,
    ) -> Result<Vec<(i64, Vec<f32>)>, LibraryError> {
        let mut s = self
            .conn
            .prepare("SELECT rowid,embedding FROM fact_vecs")
            .map_err(er)?;
        s.query_map([], |r| Ok((r.get(0)?, r.get::<_, Vec<u8>>(1)?)))
            .map_err(er)?
            .map(|x| {
                let (i, b) = x.map_err(er)?;
                Ok((i, dec(&b)?))
            })
            .collect()
    }
    pub(crate) fn historical_query_candidates(
        &self,
    ) -> Result<Vec<QueryCandidateRow>, LibraryError> {
        let mut s=self.conn.prepare("SELECT q.query_id,p.fact_id,p.created_at,v.embedding,f.text FROM queries q JOIN query_vecs v ON v.rowid=q.query_id JOIN pair_relations p ON p.query_id=q.query_id JOIN facts f ON f.fact_id=p.fact_id").map_err(er)?;
        s.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get::<_, Vec<u8>>(3)?,
                r.get(4)?,
            ))
        })
        .map_err(er)?
        .map(|x| {
            let (q, f, c, b, t) = x.map_err(er)?;
            Ok(QueryCandidateRow(q, f, c, dec(&b)?, t))
        })
        .collect()
    }
}
impl HashStore for SqliteStore {
    fn find_by_hash(&self, _: &str) -> Result<Option<i64>, LibraryError> {
        Ok(None)
    }
    fn insert(&mut self, _: &str, t: &str, _: &str, v: &[f32]) -> Result<i64, LibraryError> {
        let tx = self.conn.transaction().map_err(er)?;
        tx.execute(
            "INSERT INTO facts(text,created_at)VALUES(?1,?2)",
            rusqlite::params![t, now()],
        )
        .map_err(er)?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO fact_vecs(rowid,embedding)VALUES(?1,?2)",
            rusqlite::params![id, enc(v)],
        )
        .map_err(er)?;
        tx.commit().map_err(er)?;
        Ok(id)
    }
}
