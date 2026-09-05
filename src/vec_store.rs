//! v2 paired storage.
use crate::{
    error::LibraryError,
    ingest::{HashStore, INGEST_MODEL, INGEST_MODEL_REVISION},
};
use rusqlite::OptionalExtension;
use std::path::{Path, PathBuf};
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
        tx.execute_batch("PRAGMA foreign_keys=ON;CREATE TABLE tasks(task_id INTEGER PRIMARY KEY AUTOINCREMENT,text TEXT NOT NULL,created_at INTEGER NOT NULL,updated_at INTEGER NULL);CREATE TABLE rules(rule_id INTEGER PRIMARY KEY AUTOINCREMENT,text TEXT NOT NULL,created_at INTEGER NOT NULL,updated_at INTEGER NULL);CREATE VIRTUAL TABLE task_vecs USING vec0(embedding float[384] distance_metric=cosine);CREATE VIRTUAL TABLE rule_vecs USING vec0(embedding float[384] distance_metric=cosine);CREATE TABLE task_rule_relations(task_id INTEGER REFERENCES tasks(task_id),rule_id INTEGER REFERENCES rules(rule_id),created_at INTEGER,PRIMARY KEY(task_id,rule_id));CREATE TABLE model_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);").map_err(er)?;
        for (k, v) in [
            ("schema_version", "1"),
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
            "tasks",
            "rules",
            "task_vecs",
            "rule_vecs",
            "task_rule_relations",
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
            let vec_shadow = ["task_vecs", "rule_vecs"].iter().any(|prefix| {
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
        for owner in ["tasks", "rules"] {
            let columns = self
                .conn
                .prepare(&format!("PRAGMA table_info({owner})"))
                .map_err(er)?
                .query_map([], |r| r.get::<_, String>(1))
                .map_err(er)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(er)?;
            if !columns.iter().any(|c| c == "updated_at") {
                return Err(er("index incompatible"));
            }
        }
        for t in ["task_vecs", "rule_vecs"] {
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
        task_threshold: f64,
        rule_threshold: f64,
    ) -> Result<(i64, i64, i64, bool), LibraryError> {
        let tx = self.conn.transaction().map_err(er)?;
        let t = now();
        let nearest =
            |table: &str, vector: &[f32], threshold: f64| -> Result<Option<i64>, LibraryError> {
                let sql = format!("SELECT rowid,embedding FROM {table}");
                let mut st = tx.prepare(&sql).map_err(er)?;
                let mut best: Option<(f64, i64)> = None;
                let rows = st
                    .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))
                    .map_err(er)?;
                for row in rows {
                    let (id, bytes) = row.map_err(er)?;
                    let d = crate::query::cosine_distance(vector, &dec(&bytes)?);
                    if d <= threshold && best.as_ref().is_none_or(|b| (d, id) < (b.0, b.1)) {
                        best = Some((d, id));
                    }
                }
                Ok(best.map(|(_, id)| id))
            };
        let fi = match nearest("task_vecs", fv, task_threshold)? {
            Some(id) => id,
            None => {
                tx.execute(
                    "INSERT INTO tasks(text,created_at,updated_at)VALUES(?1,?2,NULL)",
                    rusqlite::params![f, t],
                )
                .map_err(er)?;
                let id = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO task_vecs(rowid,embedding)VALUES(?1,?2)",
                    rusqlite::params![id, enc(fv)],
                )
                .map_err(er)?;
                id
            }
        };
        let qi = match nearest("rule_vecs", qv, rule_threshold)? {
            Some(id) => id,
            None => {
                tx.execute(
                    "INSERT INTO rules(text,created_at,updated_at)VALUES(?1,?2,NULL)",
                    rusqlite::params![q, t],
                )
                .map_err(er)?;
                let id = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO rule_vecs(rowid,embedding)VALUES(?1,?2)",
                    rusqlite::params![id, enc(qv)],
                )
                .map_err(er)?;
                id
            }
        };
        let existing: Option<i64> = tx
            .query_row(
                "SELECT created_at FROM task_rule_relations WHERE task_id=?1 AND rule_id=?2",
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
            "INSERT INTO task_rule_relations VALUES(?1,?2,?3)",
            rusqlite::params![fi, qi, t],
        )
        .map_err(er)?;
        tx.commit().map_err(er)?;
        Ok((fi, qi, t, false))
    }
    pub(crate) fn read_entity_text(
        &self,
        entity: crate::command::Entity,
        id: i64,
    ) -> Result<String, LibraryError> {
        let (table, key) = match entity {
            crate::command::Entity::Rules => ("rules", "rule_id"),
            crate::command::Entity::Tasks => ("tasks", "task_id"),
        };
        self.conn
            .query_row(
                &format!("SELECT text FROM {table} WHERE {key}=?1"),
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(er)?
            .ok_or_else(|| LibraryError::MissingDefinition { id: id.to_string() })
    }
    pub(crate) fn insert_task_for_rule(
        &mut self,
        task_text: &str,
        task_embedding: &[f32],
        rule_id: i64,
    ) -> Result<(), LibraryError> {
        let tx = self.conn.transaction().map_err(er)?;
        let t = now();
        tx.execute(
            "INSERT INTO tasks(text,created_at)VALUES(?1,?2)",
            rusqlite::params![task_text, t],
        )
        .map_err(er)?;
        let task_id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO task_vecs(rowid,embedding)VALUES(?1,?2)",
            rusqlite::params![task_id, enc(task_embedding)],
        )
        .map_err(er)?;
        tx.execute(
            "INSERT OR IGNORE INTO task_rule_relations VALUES(?1,?2,?3)",
            rusqlite::params![task_id, rule_id, t],
        )
        .map_err(er)?;
        tx.commit().map_err(er)
    }
    pub(crate) fn read_entity(
        &self,
        entity: crate::command::Entity,
        id: i64,
    ) -> Result<(String, i64), LibraryError> {
        let (table, key) = match entity {
            crate::command::Entity::Rules => ("rules", "rule_id"),
            crate::command::Entity::Tasks => ("tasks", "task_id"),
        };
        self.conn
            .query_row(
                &format!("SELECT text,created_at FROM {table} WHERE {key}=?1"),
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(er)?
            .ok_or_else(|| LibraryError::MissingDefinition { id: id.to_string() })
    }
    pub(crate) fn list_entities(
        &mut self,
        entities: &[crate::command::Entity],
    ) -> Result<Vec<(i64, String, i64)>, LibraryError> {
        let tx = self.conn.transaction().map_err(er)?;
        let mut out = Vec::new();
        for entity in entities {
            let (table, key) = match entity {
                crate::command::Entity::Rules => ("rules", "rule_id"),
                crate::command::Entity::Tasks => ("tasks", "task_id"),
            };
            let mut stmt = tx
                .prepare(&format!(
                    "SELECT {key},text,created_at FROM {table} ORDER BY {key}"
                ))
                .map_err(er)?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .map_err(er)?;
            out.extend(
                rows.map(|row| row.map_err(er))
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        tx.commit().map_err(er)?;
        Ok(out)
    }
    #[cfg(test)]
    pub(crate) fn insert_status_test_row(
        &self,
        table: &str,
        text: &str,
        created: i64,
        updated: Option<i64>,
    ) -> Result<(), LibraryError> {
        self.conn
            .execute(
                &format!("INSERT INTO {table}(text,created_at,updated_at) VALUES(?1,?2,?3)"),
                rusqlite::params![text, created, updated],
            )
            .map_err(er)?;
        Ok(())
    }
    pub(crate) fn status_rows(
        &mut self,
    ) -> Result<
        (
            Vec<crate::staleness::FreshnessRow>,
            Vec<crate::staleness::FreshnessRow>,
        ),
        LibraryError,
    > {
        let tx = self.conn.transaction().map_err(er)?;
        let read =
            |table: &str, key: &str| -> Result<Vec<crate::staleness::FreshnessRow>, LibraryError> {
                let mut stmt = tx
                    .prepare(&format!(
                        "SELECT {key},text,created_at,updated_at FROM {table} ORDER BY {key}"
                    ))
                    .map_err(er)?;
                stmt.query_map([], |r| {
                    Ok(crate::staleness::FreshnessRow {
                        id: r.get(0)?,
                        text: r.get(1)?,
                        created_at: r.get(2)?,
                        updated_at: r.get(3)?,
                    })
                })
                .map_err(er)?
                .map(|r| r.map_err(er))
                .collect()
            };
        let tasks = read("tasks", "task_id")?;
        let rules = read("rules", "rule_id")?;
        tx.commit().map_err(er)?;
        Ok((tasks, rules))
    }

    pub(crate) fn remove_record(&mut self, id: i64) -> Result<(), LibraryError> {
        let tx = self.conn.transaction().map_err(er)?;
        tx.execute("DELETE FROM task_rule_relations WHERE task_id=?1", [id])
            .map_err(er)?;
        tx.execute("DELETE FROM task_vecs WHERE rowid=?1", [id])
            .map_err(er)?;
        if tx
            .execute("DELETE FROM tasks WHERE task_id=?1", [id])
            .map_err(er)?
            != 1
        {
            return Err(LibraryError::MissingDefinition { id: id.to_string() });
        };
        tx.commit().map_err(er)
    }
    pub(crate) fn apply_patch(
        &mut self,
        entity: crate::command::Entity,
        id: i64,
        text: &str,
        v: Option<&[f32]>,
        threshold: f64,
    ) -> Result<(), LibraryError> {
        let tx = self.conn.transaction().map_err(er)?;
        let (table, key, vec_table, rel_key) = match entity {
            crate::command::Entity::Rules => ("rules", "rule_id", "rule_vecs", "rule_id"),
            crate::command::Entity::Tasks => ("tasks", "task_id", "task_vecs", "task_id"),
        };
        if tx
            .execute(
                &format!("UPDATE {table} SET text=?1,updated_at=CASE WHEN text<>?1 THEN ?3 ELSE updated_at END WHERE {key}=?2"),
                rusqlite::params![text, id, now()],
            )
            .map_err(er)?
            != 1
        {
            return Err(LibraryError::MissingDefinition { id: id.to_string() });
        };
        if let Some(v) = v {
            let mut candidate = None;
            let mut st = tx
                .prepare(&format!(
                    "SELECT rowid,embedding FROM {vec_table} WHERE rowid<>?1"
                ))
                .map_err(er)?;
            let rows = st
                .query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))
                .map_err(er)?;
            for row in rows {
                let (other, bytes) = row.map_err(er)?;
                let distance = crate::query::cosine_distance(v, &dec(&bytes)?);
                if distance <= threshold
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
                    &format!(
                        "UPDATE OR IGNORE task_rule_relations SET {rel_key}=?1 WHERE {rel_key}=?2"
                    ),
                    rusqlite::params![canonical, id],
                )
                .map_err(er)?;
                tx.execute(
                    &format!("DELETE FROM task_rule_relations WHERE {rel_key}=?1"),
                    [id],
                )
                .map_err(er)?;
                tx.execute(&format!("DELETE FROM {vec_table} WHERE rowid=?1"), [id])
                    .map_err(er)?;
                let (delete_table, delete_key) = match entity {
                    crate::command::Entity::Rules => ("rules", "rule_id"),
                    crate::command::Entity::Tasks => ("tasks", "task_id"),
                };
                tx.execute(
                    &format!("DELETE FROM {delete_table} WHERE {delete_key}=?1"),
                    [id],
                )
                .map_err(er)?;
            } else {
                tx.execute(&format!("DELETE FROM {vec_table} WHERE rowid=?1"), [id])
                    .map_err(er)?;
                tx.execute(
                    &format!("INSERT INTO {vec_table}(rowid,embedding)VALUES(?1,?2)"),
                    rusqlite::params![id, enc(v)],
                )
                .map_err(er)?;
            }
        }
        tx.commit().map_err(er)
    }
    #[allow(dead_code)]
    pub(crate) fn scan_filtered_embeddings(&self) -> Result<Vec<(i64, Vec<f32>)>, LibraryError> {
        let mut s = self
            .conn
            .prepare("SELECT rowid,embedding FROM task_vecs")
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
        let mut s = self.conn.prepare("SELECT q.rule_id,p.task_id,p.created_at,v.embedding,q.text FROM rules q JOIN task_rule_relations p ON p.rule_id=q.rule_id JOIN task_vecs v ON v.rowid=p.task_id").map_err(er)?;
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
    fn insert(&mut self, _: &str, t: &str, v: &[f32]) -> Result<i64, LibraryError> {
        let tx = self.conn.transaction().map_err(er)?;
        tx.execute(
            "INSERT INTO tasks(text,created_at)VALUES(?1,?2)",
            rusqlite::params![t, now()],
        )
        .map_err(er)?;
        let id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO task_vecs(rowid,embedding)VALUES(?1,?2)",
            rusqlite::params![id, enc(v)],
        )
        .map_err(er)?;
        tx.commit().map_err(er)?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    fn root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "infraware-store-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    #[test]
    fn fresh_owner_columns_are_nullable() {
        let p = root();
        let s = SqliteStore::open(&p).unwrap();
        for table in ["tasks", "rules"] {
            let (n,t):(i64,String)=s.conn.query_row(&format!("SELECT \"notnull\", \"type\" FROM pragma_table_info('{table}') WHERE name='updated_at'"),[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
            assert_eq!(n, 0);
            assert_eq!(t, "INTEGER");
        }
        drop(s);
        let _ = std::fs::remove_dir_all(p);
    }
    #[test]
    fn old_store_is_rejected_without_repair() {
        let p = root();
        std::fs::create_dir_all(p.join(".infraware")).unwrap();
        let db = rusqlite::Connection::open(p.join(".infraware/vec-index.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE tasks(task_id INTEGER PRIMARY KEY,text TEXT NOT NULL,created_at INTEGER NOT NULL); CREATE TABLE rules(rule_id INTEGER PRIMARY KEY,text TEXT NOT NULL,created_at INTEGER NOT NULL); CREATE TABLE task_vecs(x); CREATE TABLE rule_vecs(x); CREATE TABLE task_rule_relations(x); CREATE TABLE model_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);").unwrap();
        drop(db);
        assert!(SqliteStore::open_read_only(&p).is_err());
        let db = rusqlite::Connection::open(p.join(".infraware/vec-index.sqlite")).unwrap();
        let n: i64 = db
            .query_row(
                "SELECT count(*) FROM pragma_table_info('tasks') WHERE name='updated_at'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
        let _ = std::fs::remove_dir_all(p);
    }
}

#[cfg(test)]
mod revision_tests {
    use super::*;
    use crate::command::Entity;
    use std::time::{SystemTime, UNIX_EPOCH};
    fn root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "infraware-revision-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    #[test]
    fn changed_patch_sets_timestamp_and_noop_preserves_it() {
        let p = root();
        let mut s = SqliteStore::open(&p).unwrap();
        s.conn
            .execute(
                "INSERT INTO rules(text,created_at,updated_at) VALUES('old',10,NULL)",
                [],
            )
            .unwrap();
        let id = s.conn.last_insert_rowid();
        s.apply_patch(Entity::Rules, id, "new", None, 0.).unwrap();
        let first: i64 = s
            .conn
            .query_row("SELECT updated_at FROM rules WHERE rule_id=?", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(first >= 10);
        s.apply_patch(Entity::Rules, id, "new", None, 0.).unwrap();
        let second: i64 = s
            .conn
            .query_row("SELECT updated_at FROM rules WHERE rule_id=?", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(first, second);
        drop(s);
        let _ = std::fs::remove_dir_all(p);
    }
    #[test]
    fn failed_vector_work_rolls_back_text_and_timestamp() {
        let p = root();
        let mut s = SqliteStore::open(&p).unwrap();
        s.conn
            .execute(
                "INSERT INTO tasks(text,created_at,updated_at) VALUES('old',10,NULL)",
                [],
            )
            .unwrap();
        let id = s.conn.last_insert_rowid();
        s.conn
            .execute(
                "INSERT INTO task_vecs(rowid,embedding) VALUES(?,?)",
                rusqlite::params![id, vec![0u8; 1536]],
            )
            .unwrap();
        let result = s.apply_patch(Entity::Tasks, id, "new", Some(&[1.0]), 0.);
        assert!(result.is_err());
        let text: String = s
            .conn
            .query_row("SELECT text FROM tasks WHERE task_id=?", [id], |r| r.get(0))
            .unwrap();
        let updated: Option<i64> = s
            .conn
            .query_row("SELECT updated_at FROM tasks WHERE task_id=?", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(text, "old");
        assert_eq!(updated, None);
        drop(s);
        let _ = std::fs::remove_dir_all(p);
    }
}
