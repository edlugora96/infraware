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

pub(crate) struct GroupedRulePage {
    pub(crate) total: usize,
    pub(crate) limit: usize,
    pub(crate) offset: usize,
    pub(crate) rules: Vec<GroupedRule>,
}

pub(crate) struct GroupedRule {
    pub(crate) id: i64,
    pub(crate) text: String,
    pub(crate) created_at: i64,
    pub(crate) updated_at: Option<i64>,
    pub(crate) tasks: Vec<GroupedTask>,
}

pub(crate) struct GroupedTask {
    pub(crate) id: i64,
    pub(crate) text: String,
    pub(crate) created_at: i64,
    pub(crate) updated_at: Option<i64>,
}

pub(crate) struct RelatedRecord {
    pub(crate) id: i64,
    pub(crate) text: String,
    pub(crate) created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoveFailureStage {
    Validation,
    RelationDeletion,
    OrphanDetection,
    TaskVectorCleanup,
    RuleVectorCleanup,
    CommitPreparation,
}

#[cfg(test)]
fn fail_remove_at(
    configured: Option<RemoveFailureStage>,
    stage: RemoveFailureStage,
) -> Result<(), LibraryError> {
    if configured == Some(stage) {
        Err(er("injected remove failure"))
    } else {
        Ok(())
    }
}

#[cfg(not(test))]
fn fail_remove_at(_: RemoveFailureStage) -> Result<(), LibraryError> {
    Ok(())
}

pub(crate) struct SqliteStore {
    conn: rusqlite::Connection,
    #[cfg(test)]
    remove_failure: Option<RemoveFailureStage>,
}
impl SqliteStore {
    pub(crate) fn open(r: &Path) -> Result<Self, LibraryError> {
        std::fs::create_dir_all(r.join(".infraware")).map_err(er)?;
        let fresh = !path(r).exists();
        crate::vec_shim::ensure_registered();
        let mut s = Self {
            conn: rusqlite::Connection::open(path(r)).map_err(er)?,
            #[cfg(test)]
            remove_failure: None,
        };
        if fresh {
            s.init()?;
        }
        s.verify_query_compatible()?;
        Ok(s)
    }
    pub(crate) fn open_existing_rw(r: &Path) -> Result<Self, LibraryError> {
        let store_path = path(r);
        let metadata = match std::fs::symlink_metadata(&store_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(LibraryError::UninitializedStorage);
            }
            Err(_) => return Err(er("index unavailable")),
        };
        if metadata.file_type().is_symlink() {
            if !std::fs::metadata(&store_path)
                .map(|target| target.is_file())
                .unwrap_or(false)
            {
                return Err(er("index unavailable"));
            }
        } else if !metadata.is_file() {
            return Err(er("index unavailable"));
        }
        crate::vec_shim::ensure_registered();
        let s = Self {
            conn: rusqlite::Connection::open(path(r)).map_err(er)?,
            #[cfg(test)]
            remove_failure: None,
        };
        s.verify_query_compatible()?;
        Ok(s)
    }
    pub(crate) fn open_read_only(r: &Path) -> Result<Self, LibraryError> {
        match std::fs::symlink_metadata(path(r)) {
            Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(er("index unavailable")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(LibraryError::UninitializedStorage);
            }
            Err(_) => return Err(er("index unavailable")),
        }
        crate::vec_shim::ensure_registered();
        let s = Self {
            conn: rusqlite::Connection::open_with_flags(
                path(r),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(er)?,
            #[cfg(test)]
            remove_failure: None,
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
    pub(crate) fn related_records(
        &self,
        entity: crate::command::Entity,
        id: i64,
    ) -> Result<(usize, Vec<RelatedRecord>), LibraryError> {
        let (count_sql, rows_sql) = match entity {
            crate::command::Entity::Rules => (
                "SELECT count(*) FROM task_rule_relations p JOIN tasks t ON t.task_id=p.task_id WHERE p.rule_id=?1",
                "SELECT t.task_id,t.text,t.created_at FROM task_rule_relations p JOIN tasks t ON t.task_id=p.task_id WHERE p.rule_id=?1 ORDER BY t.created_at ASC,t.task_id ASC LIMIT 500",
            ),
            crate::command::Entity::Tasks => (
                "SELECT count(*) FROM task_rule_relations p JOIN rules r ON r.rule_id=p.rule_id WHERE p.task_id=?1",
                "SELECT r.rule_id,r.text,r.created_at FROM task_rule_relations p JOIN rules r ON r.rule_id=p.rule_id WHERE p.task_id=?1 ORDER BY r.created_at ASC,r.rule_id ASC LIMIT 500",
            ),
        };
        let count: i64 = self
            .conn
            .query_row(count_sql, [id], |r| r.get(0))
            .map_err(er)?;
        let mut statement = self.conn.prepare(rows_sql).map_err(er)?;
        let records = statement
            .query_map([id], |r| {
                Ok(RelatedRecord {
                    id: r.get(0)?,
                    text: r.get(1)?,
                    created_at: r.get(2)?,
                })
            })
            .map_err(er)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(er)?;
        Ok((usize::try_from(count).map_err(er)?, records))
    }

    pub(crate) fn grouped_rule_page(
        &mut self,
        requested_limit: usize,
        offset: usize,
    ) -> Result<GroupedRulePage, LibraryError> {
        let limit = requested_limit.min(500);
        let limit_sql = i64::try_from(limit).map_err(er)?;
        let offset_sql = i64::try_from(offset).map_err(er)?;
        let tx = self.conn.transaction().map_err(er)?;

        let total: i64 = tx
            .query_row("SELECT count(*) FROM rules", [], |row| row.get(0))
            .map_err(er)?;
        let mut rules = tx
            .prepare(
                "SELECT rule_id,text,created_at,updated_at FROM rules \
                 ORDER BY created_at ASC, rule_id ASC LIMIT ?1 OFFSET ?2",
            )
            .map_err(er)?
            .query_map(rusqlite::params![limit_sql, offset_sql], |row| {
                Ok(GroupedRule {
                    id: row.get(0)?,
                    text: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                    tasks: Vec::new(),
                })
            })
            .map_err(er)?
            .map(|row| row.map_err(er))
            .collect::<Result<Vec<_>, _>>()?;

        if !rules.is_empty() {
            let placeholders = std::iter::repeat_n("?", rules.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT r.rule_id,t.task_id,t.text,t.created_at,t.updated_at \
                 FROM task_rule_relations r JOIN tasks t ON t.task_id=r.task_id \
                 WHERE r.rule_id IN ({placeholders}) \
                 ORDER BY r.rule_id ASC, t.created_at ASC, t.task_id ASC"
            );
            let ids = rules.iter().map(|rule| rule.id).collect::<Vec<_>>();
            let mut stmt = tx.prepare(&sql).map_err(er)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(ids), |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        GroupedTask {
                            id: row.get(1)?,
                            text: row.get(2)?,
                            created_at: row.get(3)?,
                            updated_at: row.get(4)?,
                        },
                    ))
                })
                .map_err(er)?;
            for row in rows {
                let (rule_id, task) = row.map_err(er)?;
                if let Some(rule) = rules.iter_mut().find(|rule| rule.id == rule_id) {
                    rule.tasks.push(task);
                }
            }
        }

        tx.commit().map_err(er)?;
        Ok(GroupedRulePage {
            total: usize::try_from(total).map_err(er)?,
            limit,
            offset,
            rules,
        })
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

    #[cfg(test)]
    fn set_remove_failure(&mut self, stage: RemoveFailureStage) {
        self.remove_failure = Some(stage);
    }

    pub(crate) fn remove_record(
        &mut self,
        entity: crate::command::Entity,
        id: i64,
    ) -> Result<(), LibraryError> {
        let (table, key, vec_table, relation_key) = match entity {
            crate::command::Entity::Rules => ("rules", "rule_id", "rule_vecs", "rule_id"),
            crate::command::Entity::Tasks => ("tasks", "task_id", "task_vecs", "task_id"),
        };
        #[cfg(test)]
        let remove_failure = self.remove_failure;
        let tx = self.conn.transaction().map_err(er)?;
        fail_remove_at(
            #[cfg(test)]
            remove_failure,
            RemoveFailureStage::Validation,
        )?;
        let exists: Option<i64> = tx
            .query_row(
                &format!("SELECT {key} FROM {table} WHERE {key}=?1"),
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(er)?;
        if exists.is_none() {
            return Err(LibraryError::MissingDefinition { id: id.to_string() });
        }
        let linked_task_ids = if matches!(entity, crate::command::Entity::Rules) {
            let mut statement = tx
                .prepare("SELECT task_id FROM task_rule_relations WHERE rule_id=?1")
                .map_err(er)?;
            statement
                .query_map([id], |row| row.get::<_, i64>(0))
                .map_err(er)?
                .map(|row| row.map_err(er))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        fail_remove_at(
            #[cfg(test)]
            remove_failure,
            RemoveFailureStage::RelationDeletion,
        )?;
        tx.execute(
            &format!("DELETE FROM task_rule_relations WHERE {relation_key}=?1"),
            [id],
        )
        .map_err(er)?;

        if matches!(entity, crate::command::Entity::Rules) {
            for task_id in linked_task_ids {
                fail_remove_at(
                    #[cfg(test)]
                    remove_failure,
                    RemoveFailureStage::OrphanDetection,
                )?;
                let has_relations: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM task_rule_relations WHERE task_id=?1)",
                        [task_id],
                        |row| row.get(0),
                    )
                    .map_err(er)?;
                if !has_relations {
                    fail_remove_at(
                        #[cfg(test)]
                        remove_failure,
                        RemoveFailureStage::TaskVectorCleanup,
                    )?;
                    tx.execute("DELETE FROM task_vecs WHERE rowid=?1", [task_id])
                        .map_err(er)?;
                    tx.execute("DELETE FROM tasks WHERE task_id=?1", [task_id])
                        .map_err(er)?;
                }
            }
        }

        fail_remove_at(
            #[cfg(test)]
            remove_failure,
            RemoveFailureStage::RuleVectorCleanup,
        )?;
        tx.execute(&format!("DELETE FROM {vec_table} WHERE rowid=?1"), [id])
            .map_err(er)?;
        if tx
            .execute(&format!("DELETE FROM {table} WHERE {key}=?1"), [id])
            .map_err(er)?
            != 1
        {
            return Err(LibraryError::MissingDefinition { id: id.to_string() });
        }
        fail_remove_at(
            #[cfg(test)]
            remove_failure,
            RemoveFailureStage::CommitPreparation,
        )?;
        tx.commit().map_err(er)
    }
    pub(crate) fn revise(
        &mut self,
        entity: crate::command::Entity,
        id: i64,
    ) -> Result<(), LibraryError> {
        let (table, key) = match entity {
            crate::command::Entity::Rules => ("rules", "rule_id"),
            crate::command::Entity::Tasks => ("tasks", "task_id"),
        };
        if self
            .conn
            .execute(
                &format!("UPDATE {table} SET updated_at=?1 WHERE {key}=?2"),
                rusqlite::params![now(), id],
            )
            .map_err(er)?
            != 1
        {
            return Err(LibraryError::MissingDefinition { id: id.to_string() });
        }
        Ok(())
    }

    pub(crate) fn apply_patch(
        &mut self,
        entity: crate::command::Entity,
        id: i64,
        text: &str,
        v: Option<&[f32]>,
        threshold: f64,
    ) -> Result<bool, LibraryError> {
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
        let mut duplicate = false;
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
            duplicate = candidate.is_some();
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
        tx.commit().map_err(er)?;
        Ok(duplicate)
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
    fn absent_existing_paths_are_typed_and_do_not_create_storage() {
        let p = root();
        assert!(matches!(
            SqliteStore::open_existing_rw(&p),
            Err(LibraryError::UninitializedStorage)
        ));
        assert!(matches!(
            SqliteStore::open_read_only(&p),
            Err(LibraryError::UninitializedStorage)
        ));
        assert!(!p.join(".infraware").exists());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_store_symlink_stays_generic_and_does_not_create_target() {
        use std::os::unix::fs::symlink;

        let p = root();
        std::fs::create_dir_all(p.join(".infraware")).unwrap();
        let target = p.join("missing.sqlite");
        symlink(&target, p.join(".infraware/vec-index.sqlite")).unwrap();

        assert!(matches!(
            SqliteStore::open_existing_rw(&p),
            Err(LibraryError::Io { .. })
        ));
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(p);
    }

    #[test]
    fn present_malformed_store_stays_generic() {
        let p = root();
        std::fs::create_dir_all(p.join(".infraware")).unwrap();
        std::fs::write(p.join(".infraware/vec-index.sqlite"), b"not sqlite").unwrap();
        assert!(matches!(
            SqliteStore::open_existing_rw(&p),
            Err(LibraryError::Io { .. })
        ));
        assert!(matches!(
            SqliteStore::open_read_only(&p),
            Err(LibraryError::Io { .. })
        ));
        let _ = std::fs::remove_dir_all(p);
    }

    #[test]
    fn removal_keeps_existing_explicit_schema_unchanged() {
        let p = root();
        let store = SqliteStore::open(&p).unwrap();
        let schema_version: String = store
            .conn
            .query_row(
                "SELECT value FROM model_meta WHERE key='schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(schema_version, "1");
        let relation_sql: String = store
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='task_rule_relations'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            !relation_sql
                .to_ascii_uppercase()
                .contains("ON DELETE CASCADE")
        );
        assert!(relation_sql.contains("REFERENCES tasks"));
        assert!(relation_sql.contains("REFERENCES rules"));
        drop(store);
        let reopened = SqliteStore::open_existing_rw(&p).unwrap();
        drop(reopened);
        let _ = std::fs::remove_dir_all(p);
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
    fn grouped_rule_pages_use_explicit_relations_and_entity_ordering() {
        let p = root();
        let mut store = SqliteStore::open(&p).unwrap();

        // Keep the fixture rows explicit. Relation timestamps intentionally
        // disagree with the entity timestamps so the assertions catch an
        // accidental relation-order sort.
        for (id, text, created_at) in [
            (1, "rule one", 100),
            (2, "rule two", 100),
            (3, "rule without tasks", 200),
            (4, "rule outside page", 300),
        ] {
            store
                .conn
                .execute(
                    "INSERT INTO rules(rule_id,text,created_at,updated_at) VALUES(?1,?2,?3,NULL)",
                    rusqlite::params![id, text, created_at],
                )
                .unwrap();
        }
        for (id, text, created_at) in [
            (1, "shared early task", 50),
            (2, "shared late task", 50),
            (3, "unrelated task", 70),
        ] {
            store
                .conn
                .execute(
                    "INSERT INTO tasks(task_id,text,created_at,updated_at) VALUES(?1,?2,?3,NULL)",
                    rusqlite::params![id, text, created_at],
                )
                .unwrap();
        }
        for (task_id, rule_id, relation_created_at) in [
            (2, 1, 10), // later relation row, but task ID tie-breaks first/second
            (1, 1, 20),
            (1, 2, 30), // task 1 is reused under both returned rules
            (2, 2, 40),
            (3, 4, 1), // relation belongs to a rule outside the selected page
        ] {
            store
                .conn
                .execute(
                    "INSERT INTO task_rule_relations(task_id,rule_id,created_at) VALUES(?1,?2,?3)",
                    rusqlite::params![task_id, rule_id, relation_created_at],
                )
                .unwrap();
        }

        let first_page = store.grouped_rule_page(2, 0).unwrap();
        assert_eq!(first_page.total, 4);
        assert_eq!(first_page.limit, 2);
        assert_eq!(first_page.offset, 0);
        assert_eq!(
            first_page
                .rules
                .iter()
                .map(|rule| rule.id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(
            first_page.rules[0]
                .tasks
                .iter()
                .map(|task| task.id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(
            first_page.rules[1]
                .tasks
                .iter()
                .map(|task| task.id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(first_page.rules[0].tasks[0].text, "shared early task");
        assert_eq!(first_page.rules[1].tasks[0].text, "shared early task");

        // Nested tasks are complete for a rule page. They are not paged, and
        // relations from rules outside that page cannot leak into it.
        let second_page = store.grouped_rule_page(1, 1).unwrap();
        assert_eq!(second_page.total, 4);
        assert_eq!(second_page.rules.len(), 1);
        assert_eq!(second_page.rules[0].id, 2);
        assert_eq!(
            second_page.rules[0]
                .tasks
                .iter()
                .map(|task| task.id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(!second_page.rules[0].tasks.iter().any(|task| task.id == 3));

        let empty_rule_page = store.grouped_rule_page(1, 2).unwrap();
        assert_eq!(empty_rule_page.total, 4);
        assert_eq!(empty_rule_page.rules[0].id, 3);
        assert!(empty_rule_page.rules[0].tasks.is_empty());

        drop(store);
        let _ = std::fs::remove_dir_all(p);
    }

    #[test]
    fn remove_record_selects_typed_entity_branch() {
        let p = root();
        let mut store = SqliteStore::open(&p).unwrap();
        for (table, id, text) in [
            ("rules", 1_i64, "rule"),
            ("rules", 2_i64, "other rule"),
            ("rules", 3_i64, "empty rule"),
            ("tasks", 2_i64, "task"),
            ("tasks", 3_i64, "exclusive task"),
            ("tasks", 4_i64, "unlinked task"),
        ] {
            store
                .conn
                .execute(
                    &format!(
                        "INSERT INTO {table}({}_id,text,created_at,updated_at) VALUES(?1,?2,0,NULL)",
                        if table == "rules" { "rule" } else { "task" }
                    ),
                    rusqlite::params![id, text],
                )
                .unwrap();
        }
        store
            .conn
            .execute(
                "INSERT INTO task_rule_relations(task_id,rule_id,created_at) VALUES(2,1,0),(2,2,0),(3,1,0)",
                [],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO task_vecs(rowid,embedding) VALUES(2,?1),(3,?1),(4,?1)",
                [vec![0_u8; 1536]],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO rule_vecs(rowid,embedding) VALUES(1,?1),(2,?1)",
                [vec![0_u8; 1536]],
            )
            .unwrap();

        store
            .remove_record(crate::command::Entity::Rules, 1)
            .unwrap();
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM rules WHERE rule_id=1", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        let missing = store.remove_record(crate::command::Entity::Rules, 99);
        assert_eq!(
            missing,
            Err(LibraryError::MissingDefinition { id: "99".into() })
        );
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM rules WHERE rule_id=2", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM tasks WHERE task_id=2", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM tasks WHERE task_id=3", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM task_vecs WHERE rowid=3", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT count(*) FROM task_rule_relations WHERE task_id=2 AND rule_id=2",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );

        store
            .remove_record(crate::command::Entity::Tasks, 2)
            .unwrap();
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM tasks WHERE task_id=2", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM task_vecs WHERE rowid=2", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT count(*) FROM task_rule_relations WHERE task_id=2",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM rules WHERE rule_id=2", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM rule_vecs WHERE rowid=2", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        store
            .remove_record(crate::command::Entity::Rules, 3)
            .unwrap();
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM rules WHERE rule_id=3", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        store
            .remove_record(crate::command::Entity::Tasks, 4)
            .unwrap();
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM tasks WHERE task_id=4", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM task_vecs WHERE rowid=4", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(store);
        let _ = std::fs::remove_dir_all(p);
    }

    #[test]
    fn removal_uses_entity_when_task_and_rule_ids_collide() {
        let p = root();
        let mut store = SqliteStore::open(&p).unwrap();
        store
            .conn
            .execute(
                "INSERT INTO rules(rule_id,text,created_at,updated_at) VALUES(1,'rule',0,NULL)",
                [],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO tasks(task_id,text,created_at,updated_at) VALUES(1,'task',0,NULL)",
                [],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO task_rule_relations(task_id,rule_id,created_at) VALUES(1,1,0)",
                [],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO task_vecs(rowid,embedding) VALUES(1,?1)",
                [vec![0_u8; 1536]],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO rule_vecs(rowid,embedding) VALUES(1,?1)",
                [vec![0_u8; 1536]],
            )
            .unwrap();

        store
            .remove_record(crate::command::Entity::Tasks, 1)
            .unwrap();
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM rules WHERE rule_id=1", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM task_vecs WHERE rowid=1", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        store
            .remove_record(crate::command::Entity::Rules, 1)
            .unwrap();
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM rule_vecs WHERE rowid=1", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(store);
        let _ = std::fs::remove_dir_all(p);
    }

    #[test]
    fn injected_remove_failures_roll_back_every_stage() {
        for stage in [
            RemoveFailureStage::Validation,
            RemoveFailureStage::RelationDeletion,
            RemoveFailureStage::OrphanDetection,
            RemoveFailureStage::TaskVectorCleanup,
            RemoveFailureStage::RuleVectorCleanup,
            RemoveFailureStage::CommitPreparation,
        ] {
            let p = root();
            let mut store = SqliteStore::open(&p).unwrap();
            let embedding = vec![7_u8; 1536];
            store
                .conn
                .execute_batch(
                    "INSERT INTO rules(rule_id,text,created_at,updated_at) VALUES(1,'rule',10,20);\
                 INSERT INTO tasks(task_id,text,created_at,updated_at) VALUES(1,'task',11,21);\
                 INSERT INTO task_rule_relations(task_id,rule_id,created_at) VALUES(1,1,30);",
                )
                .unwrap();
            // Keep both vector fixtures at the schema's required byte length.
            store
                .conn
                .execute(
                    "INSERT INTO task_vecs(rowid,embedding) VALUES(1,?1)",
                    [&embedding],
                )
                .unwrap();
            store
                .conn
                .execute(
                    "INSERT INTO rule_vecs(rowid,embedding) VALUES(1,?1)",
                    [&embedding],
                )
                .unwrap();

            let before_rule: (i64, String, i64, Option<i64>) = store
                .conn
                .query_row(
                    "SELECT rule_id,text,created_at,updated_at FROM rules WHERE rule_id=1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .unwrap();
            let before_task: (i64, String, i64, Option<i64>) = store
                .conn
                .query_row(
                    "SELECT task_id,text,created_at,updated_at FROM tasks WHERE task_id=1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .unwrap();
            let before_relation: (i64, i64, i64) = store
                .conn
                .query_row(
                    "SELECT task_id,rule_id,created_at FROM task_rule_relations",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            let before_task_vector: Vec<u8> = store
                .conn
                .query_row("SELECT embedding FROM task_vecs WHERE rowid=1", [], |r| {
                    r.get::<_, Vec<u8>>(0)
                })
                .unwrap();
            let before_rule_vector: Vec<u8> = store
                .conn
                .query_row("SELECT embedding FROM rule_vecs WHERE rowid=1", [], |r| {
                    r.get::<_, Vec<u8>>(0)
                })
                .unwrap();

            store.set_remove_failure(stage);
            assert!(
                matches!(
                    store.remove_record(crate::command::Entity::Rules, 1),
                    Err(LibraryError::Io { .. })
                ),
                "failure stage {stage:?} did not fail"
            );
            assert_eq!(
                store
                    .conn
                    .query_row(
                        "SELECT rule_id,text,created_at,updated_at FROM rules WHERE rule_id=1",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .unwrap(),
                before_rule
            );
            assert_eq!(
                store
                    .conn
                    .query_row(
                        "SELECT task_id,text,created_at,updated_at FROM tasks WHERE task_id=1",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .unwrap(),
                before_task
            );
            assert_eq!(
                store
                    .conn
                    .query_row(
                        "SELECT task_id,rule_id,created_at FROM task_rule_relations",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .unwrap(),
                before_relation
            );
            assert_eq!(
                store
                    .conn
                    .query_row("SELECT embedding FROM task_vecs WHERE rowid=1", [], |r| r
                        .get::<_, Vec<
                        u8,
                    >>(
                        0
                    ),)
                    .unwrap(),
                before_task_vector
            );
            assert_eq!(
                store
                    .conn
                    .query_row("SELECT embedding FROM rule_vecs WHERE rowid=1", [], |r| r
                        .get::<_, Vec<
                        u8,
                    >>(
                        0
                    ),)
                    .unwrap(),
                before_rule_vector
            );
            drop(store);
            std::fs::remove_dir_all(&p).unwrap();
        }
    }

    #[test]
    fn related_records_are_scoped_ordered_joined_and_capped() {
        let p = root();
        let store = SqliteStore::open(&p).unwrap();
        for (id, created) in [(1, 20), (2, 10), (3, 10)] {
            store
                .conn
                .execute(
                    "INSERT INTO rules(rule_id,text,created_at) VALUES(?1,?2,?3)",
                    rusqlite::params![id, format!("rule{id}"), created],
                )
                .unwrap();
            store
                .conn
                .execute(
                    "INSERT INTO tasks(task_id,text,created_at) VALUES(?1,?2,?3)",
                    rusqlite::params![id, format!("task{id}"), created],
                )
                .unwrap();
        }
        store.conn.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        store.conn.execute("INSERT INTO task_rule_relations(task_id,rule_id,created_at) VALUES (1,1,0),(2,1,0),(3,1,0),(999,1,0)", []).unwrap();
        store.conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        let (count, rows) = store
            .related_records(crate::command::Entity::Rules, 1)
            .unwrap();
        assert_eq!(count, 3);
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2, 3, 1]);
        let (count, rows) = store
            .related_records(crate::command::Entity::Tasks, 1)
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(rows[0].id, 1);
        for id in 4..=503 {
            store
                .conn
                .execute(
                    "INSERT INTO tasks(task_id,text,created_at) VALUES(?1,?2,?1)",
                    rusqlite::params![id, format!("task{id}")],
                )
                .unwrap();
            store
                .conn
                .execute(
                    "INSERT INTO task_rule_relations(task_id,rule_id,created_at) VALUES(?1,1,0)",
                    [id],
                )
                .unwrap();
        }
        let (count, rows) = store
            .related_records(crate::command::Entity::Rules, 1)
            .unwrap();
        assert_eq!(count, 503);
        assert_eq!(rows.len(), 500);
        assert_eq!(rows[0].id, 4);
        drop(store);
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
