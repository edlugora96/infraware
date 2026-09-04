//! Lazy sqlite-vec backed [`crate::ingest::HashStore`].

use std::path::{Path, PathBuf};

use crate::error::LibraryError;
use crate::ingest::{HashStore, INGEST_DIMENSION, INGEST_MODEL, INGEST_MODEL_REVISION};

fn io_err(msg: impl std::fmt::Display) -> LibraryError {
    LibraryError::Io {
        message: msg.to_string(),
    }
}

/// Sole owner of the vector-index path (task 2.1).
///
/// The index lives at exactly `<root>/.infraware/vec-index.sqlite`. All
/// storage open paths must resolve through this helper so the location
/// cannot drift between the ingest (lazy-create) and read-only lanes.
fn vec_path(root: &Path) -> PathBuf {
    root.join(".infraware").join("vec-index.sqlite")
}

/// RAII guard for the ephemeral read snapshot directory.
///
/// Lives as long as the read-only connection; removes the temp copy on drop
/// (success, error after creation, or normal close).
struct SnapshotDir {
    path: PathBuf,
}

impl SnapshotDir {
    fn remove(&self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

impl Drop for SnapshotDir {
    fn drop(&mut self) {
        self.remove();
    }
}

pub(crate) struct SqliteStore {
    conn: rusqlite::Connection,
    /// Present only for the read-only snapshot path; keeps the temp copy
    /// alive for the connection lifetime. Ingest opener leaves it `None`.
    _snapshot: Option<SnapshotDir>,
}

/// Sibling artifacts copied alongside the main DB when present.
const SNAPSHOT_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-journal"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct FileSig {
    len: u64,
    mtime: Option<(i64, u32, u32)>,
}

fn sig_of(path: &Path) -> Option<FileSig> {
    let md = std::fs::metadata(path).ok()?;
    if !md.is_file() {
        return None;
    }
    let mtime = md.modified().ok().and_then(|t| {
        // Truncate to a coarse but stable representation for comparison.
        let d = t.duration_since(std::time::UNIX_EPOCH).ok()?;
        #[allow(clippy::cast_possible_wrap)]
        Some((d.as_secs() as i64, d.subsec_nanos(), 0u32))
    });
    Some(FileSig {
        len: md.len(),
        mtime,
    })
}

/// Snapshot the source artifact set into a fresh temp directory.
///
/// Copies the main DB plus every currently present `-wal`/`-shm`/`-journal`
/// sibling under matching basenames. Stability is checked per attempt by
/// comparing source presence/size/mtime before and after the copy; any change
/// discards the partial copy and retries, up to three immediate attempts.
fn materialize_snapshot(src_main: &Path) -> Result<SnapshotDir, LibraryError> {
    let file_name = src_main
        .file_name()
        .ok_or_else(|| io_err("vec index not found"))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    #[allow(clippy::cast_possible_truncation)]
    let unique = format!("infraware-vec-ro-{}-{}-{}", std::process::id(), stamp, {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        stamp.hash(&mut h);
        std::thread::current().id().hash(&mut h);
        h.finish()
    });
    let dir = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&dir).map_err(io_err)?;

    let mut last_err: Option<LibraryError> = None;
    for _ in 0..3 {
        // Clear any partial copy from a previous unstable attempt.
        clear_dir(&dir);
        match try_copy_once(src_main, file_name, &dir) {
            Ok(()) => return Ok(SnapshotDir { path: dir }),
            Err(e) => last_err = Some(e),
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    Err(last_err.unwrap_or_else(|| io_err("could not establish a stable vec index snapshot")))
}

fn clear_dir(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(&p);
            } else {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

fn try_copy_once(
    src_main: &Path,
    file_name: &std::ffi::OsStr,
    dir: &Path,
) -> Result<(), LibraryError> {
    // Record the source artifact set before copying.
    let mut sources: Vec<(PathBuf, PathBuf)> = Vec::new();
    let before_main = sig_of(src_main).ok_or_else(|| io_err("vec index not found"))?;
    sources.push((src_main.to_path_buf(), dir.join(file_name)));
    let src_base = src_main.as_os_str().to_owned();
    for suffix in SNAPSHOT_SUFFIXES {
        let mut s = src_base.clone();
        s.push(suffix);
        let src = PathBuf::from(s);
        if sig_of(&src).is_some() {
            let mut d = dir.join(file_name);
            let mut ds = d.as_os_str().to_owned();
            ds.push(suffix);
            d = PathBuf::from(ds);
            sources.push((src, d));
        }
    }
    let before: Vec<Option<FileSig>> = sources.iter().map(|(s, _)| sig_of(s)).collect();

    // Copy main first, then siblings.
    for (src, dst) in &sources {
        std::fs::copy(src, dst).map_err(io_err)?;
    }

    // Re-stat sources; any presence/size/mtime change means torn set.
    for ((src, dst), was) in sources.iter().zip(before.iter()) {
        let now = sig_of(src);
        if now != *was {
            return Err(io_err("vec index changed during snapshot"));
        }
        // Destination must match the pre-copy source length.
        if let Some(w) = was {
            let got = std::fs::metadata(dst).map_err(io_err)?.len();
            if got != w.len {
                return Err(io_err("vec index changed during snapshot"));
            }
        }
    }
    let _ = before_main;
    Ok(())
}

/// Decode one stored embedding blob: exactly 384 little-endian f32.
fn decode_embedding_384(blob: &[u8]) -> Result<Vec<f32>, String> {
    if blob.len() != INGEST_DIMENSION as usize * 4 {
        return Err(format!("invalid embedding blob length {}", blob.len()));
    }
    let mut out = Vec::with_capacity(INGEST_DIMENSION as usize);
    for chunk in blob.chunks_exact(4) {
        let arr: [u8; 4] = [chunk[0], chunk[1], chunk[2], chunk[3]];
        let v = f32::from_le_bytes(arr);
        if !v.is_finite() {
            return Err("non-finite embedding value".to_string());
        }
        out.push(v);
    }
    Ok(out)
}

impl SqliteStore {
    pub(crate) fn open(root: &Path) -> Result<Self, LibraryError> {
        let dir = root.join(".infraware");
        std::fs::create_dir_all(&dir).map_err(io_err)?;
        crate::vec_shim::ensure_registered();
        let conn = rusqlite::Connection::open(vec_path(root)).map_err(io_err)?;
        let this = Self {
            conn,
            _snapshot: None,
        };
        this.init_schema()?;
        Ok(this)
    }

    /// Read-only query constructor (task 4.1, snapshot resolution).
    ///
    /// Never opens the original with SQLite. Checks the main file exists
    /// first (creating nothing on miss), copies the main DB plus any present
    /// `-wal`/`-shm`/`-journal` siblings into a fresh system-temp directory
    /// with three immediate stability attempts, then opens only the temp copy
    /// with `SQLITE_OPEN_READ_ONLY`. No `immutable=1`: the snapshot is a
    /// consistent WAL set, so the normal read path sees committed data.
    /// Performs no DDL, transactions, or writes on either location.
    #[allow(dead_code)] // wired by later query tasks (4.2+); covered by unit test
    pub(crate) fn open_read_only(root: &Path) -> Result<Self, LibraryError> {
        let path = vec_path(root);
        if !path.is_file() {
            return Err(io_err("vec index not found"));
        }
        let file_name = path
            .file_name()
            .ok_or_else(|| io_err("vec index not found"))?;
        let snapshot = materialize_snapshot(&path)?;
        crate::vec_shim::ensure_registered();
        let conn = rusqlite::Connection::open_with_flags(
            snapshot.path.join(file_name),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| {
            let _ = std::fs::remove_dir_all(&snapshot.path);
            io_err(e)
        })?;
        Ok(Self {
            conn,
            _snapshot: Some(snapshot),
        })
    }

    /// Test-only accessor for the snapshot directory (task 4.4 cleanup proof).
    #[cfg(test)]
    pub(crate) fn snapshot_path(&self) -> Option<std::path::PathBuf> {
        self._snapshot.as_ref().map(|s| s.path.clone())
    }

    /// Read-only compatibility gate for the query path (task 4.2).
    ///
    /// Checks the expected query index schema and all six `model_meta`
    /// values exactly (`schema_version=1`, pinned model, pinned revision,
    /// `dimension=384`, `metric=cosine`, `normalized=1`) using the snapshot
    /// connection already held by this store. Performs `SELECT` reads only:
    /// no DDL, migration, repair, transaction, or write on either location.
    /// Must run before any model embedding work; error details stay
    /// internal (generic `LibraryError::Io` without paths, values, or
    /// vectors) for task 4.3 to map safely.
    #[allow(dead_code)] // wired by later query tasks; covered by unit test
    pub(crate) fn verify_query_compatible(&self) -> Result<(), LibraryError> {
        fn incompatible(reason: &str) -> LibraryError {
            // Internal token only: table/key name, never paths, values, or vectors.
            io_err(format!("vec index incompatible: {reason}"))
        }
        // Expected schema: required tables plus the vec0 virtual table.
        let vec_sql: String = self
            .conn
            .query_row("SELECT sql FROM sqlite_master WHERE name='vecs'", [], |r| {
                r.get(0)
            })
            .map_err(|_| incompatible("vecs table"))?;
        if !(vec_sql.contains("float[384]") && vec_sql.contains("cosine")) {
            return Err(incompatible("vecs table"));
        }
        for table in ["records", "meta_kv", "model_meta"] {
            let present: Option<String> = self
                .conn
                .query_row(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name=?1",
                    rusqlite::params![table],
                    |r| r.get(0),
                )
                .ok();
            if present.as_deref() != Some(table) {
                return Err(incompatible(match table {
                    "records" => "records table",
                    "meta_kv" => "meta_kv table",
                    _ => "model_meta table",
                }));
            }
        }
        let idx: Option<String> = self
            .conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='index' AND name='idx_meta_kv_kv'",
                [],
                |r| r.get(0),
            )
            .ok();
        if idx.as_deref() != Some("idx_meta_kv_kv") {
            return Err(incompatible("meta index"));
        }
        // All six model_meta values must match exactly.
        for (key, expected) in [
            ("schema_version", "1"),
            ("model", INGEST_MODEL),
            ("revision", INGEST_MODEL_REVISION),
            ("dimension", "384"),
            ("metric", "cosine"),
            ("normalized", "1"),
        ] {
            let got: Option<String> = self
                .conn
                .query_row(
                    "SELECT value FROM model_meta WHERE key=?1",
                    rusqlite::params![key],
                    |r| r.get(0),
                )
                .ok();
            if got.as_deref() != Some(expected) {
                return Err(incompatible(match key {
                    "schema_version" => "model_meta schema_version",
                    "model" => "model_meta model",
                    "revision" => "model_meta revision",
                    "dimension" => "model_meta dimension",
                    "metric" => "model_meta metric",
                    _ => "model_meta normalized",
                }));
            }
        }
        Ok(())
    }

    /// Fullscan read of every stored vector (proven spike strategy).
    ///
    /// Executes exactly `SELECT rowid, embedding FROM vecs` with no bound,
    /// no `vec0` distance function, and no engine-side row cap, then decodes
    /// each stored little-endian 384-f32 blob in Rust. Returns all rows in
    /// statement iteration order; callers rescore in Rust. Read-only:
    /// `SELECT` only, no DDL or writes. Must be called after embedding
    /// work so no statement is held across model execution; error details
    /// stay internal for the query layer to map safely.
    #[allow(dead_code)]
    pub(crate) fn scan_embeddings(&self) -> Result<Vec<(i64, Vec<f32>)>, LibraryError> {
        self.scan_filtered_embeddings(&std::collections::BTreeMap::new())
    }

    /// Filtered fullscan read (task 6.2): exact `meta_kv` AND-filter before rescore.
    ///
    /// Empty `filter` behaves exactly as [`scan_embeddings`](Self::scan_embeddings):
    /// every row is returned. A non-empty canonical `BTreeMap` adds one
    /// parameterized `EXISTS` clause per key/value pair, ANDed together:
    /// `SELECT rowid, embedding FROM vecs WHERE EXISTS(SELECT 1 FROM meta_kv
    /// WHERE record_id = vecs.rowid AND k = ? AND v = ?) AND ...`.
    /// Exact string equality, all pairs required, values bound as parameters
    /// (never interpolated). No engine distance call, no row cap, and no
    /// finite-k vec0 join: the full filtered set is iterated and decoded
    /// exactly like the unfiltered fullscan. Read-only `SELECT` only; the
    /// statement is created, drained, and dropped inside this call so it is
    /// never held across embedding work.
    pub(crate) fn scan_filtered_embeddings(
        &self,
        filter: &std::collections::BTreeMap<String, String>,
    ) -> Result<Vec<(i64, Vec<f32>)>, LibraryError> {
        let mut sql = String::from("SELECT rowid, embedding FROM vecs");
        // Keys come from a BTreeMap in byte order; values are bound, so
        // clause order is deterministic and injection-free.
        let mut params: Vec<&str> = Vec::with_capacity(filter.len() * 2);
        if !filter.is_empty() {
            let mut first = true;
            for (k, v) in filter.iter() {
                if first {
                    sql.push_str(" WHERE ");
                    first = false;
                } else {
                    sql.push_str(" AND ");
                }
                sql.push_str(
                    "EXISTS(SELECT 1 FROM meta_kv WHERE record_id = vecs.rowid AND k = ? AND v = ?)",
                );
                params.push(k.as_str());
                params.push(v.as_str());
            }
        }
        let mut stmt = self.conn.prepare(&sql).map_err(io_err)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params.iter()), |r| {
                let id: i64 = r.get(0)?;
                let blob: Vec<u8> = r.get(1)?;
                Ok((id, blob))
            })
            .map_err(io_err)?;
        let mut out = Vec::new();
        for row in rows {
            let (id, blob) = row.map_err(io_err)?;
            out.push((id, decode_embedding_384(&blob).map_err(io_err)?));
        }
        Ok(out)
    }

    /// Read one record's canonical metadata plus full text by row id.
    ///
    /// Runs a single scoped `SELECT text, metadata FROM records WHERE id=?1`
    /// created, drained, and dropped inside this call, so no statement is
    /// ever held across embedding work. The stored `metadata` blob is the
    /// canonical flat JSON object; it is parsed back into the byte-ordered
    /// map form. A missing row or malformed metadata is an internal error
    /// for the query layer to map to the safe operational error.
    pub(crate) fn read_record(
        &self,
        id: i64,
    ) -> Result<(std::collections::BTreeMap<String, String>, String), LibraryError> {
        let (text, canonical): (String, String) = self
            .conn
            .query_row(
                "SELECT text, metadata FROM records WHERE id=?1",
                rusqlite::params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| match e {
                // Task 5.3: a missing row is the distinct record-not-found
                // error, not the generic operational error. Query post-rank
                // reads still map this to the redacted generic error at the
                // query layer; a direct `get` surfaces it distinctly.
                rusqlite::Error::QueryReturnedNoRows => {
                    LibraryError::MissingDefinition { id: id.to_string() }
                }
                other => io_err(other),
            })?;
        let value: serde_json::Value = serde_json::from_str(&canonical).map_err(io_err)?;
        let obj = value
            .as_object()
            .ok_or_else(|| io_err("metadata not object"))?;
        let mut map = std::collections::BTreeMap::new();
        for (k, v) in obj {
            let vs = v.as_str().ok_or_else(|| io_err("metadata not flat"))?;
            map.insert(k.clone(), vs.to_string());
        }
        Ok((map, text))
    }

    /// Read-write opener for mutation commands (remove/patch).
    ///
    /// Unlike [`open`](Self::open) this never creates storage: a missing
    /// index file is an operational error and no directories or files are
    /// created, migrated, or repaired as a side effect (parent spec:
    /// remove/patch on missing storage fail; reads and mutations must not
    /// create or repair state). The extension registration still runs so an
    /// existing vec0 table resolves on this connection. No DDL runs here.
    pub(crate) fn open_existing_rw(root: &Path) -> Result<Self, LibraryError> {
        let path = vec_path(root);
        if !path.is_file() {
            return Err(io_err("vec index not found"));
        }
        crate::vec_shim::ensure_registered();
        // The file exists, so this opens read-write without creating.
        let conn = rusqlite::Connection::open(&path).map_err(io_err)?;
        Ok(Self {
            conn,
            _snapshot: None,
        })
    }

    /// Hard-delete one record across `meta_kv`, `vecs`, and `records` atomically.
    ///
    /// Uses only Spike-4-probe SQL: `DELETE FROM vecs WHERE rowid = ?1` is
    /// legal on the pinned stack (evidence item 4, `Ok(1)` observed) and the
    /// three deletes run in one `Immediate` transaction, so any pre-commit
    /// failure rolls back with no partial rows (evidence item 6). No VACUUM:
    /// space reclamation is future maintenance per the parent spec.
    /// A missing row is the distinct [`LibraryError::MissingDefinition`]
    /// (record-not-found), never the generic operational error.
    pub(crate) fn remove_record(&mut self, id: i64) -> Result<(), LibraryError> {
        let present: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM records WHERE id=?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .ok();
        if present != Some(id) {
            return Err(LibraryError::MissingDefinition { id: id.to_string() });
        }
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(io_err)?;
        let step: Result<(), LibraryError> = (|| {
            tx.execute(
                "DELETE FROM meta_kv WHERE record_id=?1",
                rusqlite::params![id],
            )
            .map_err(io_err)?;
            tx.execute("DELETE FROM vecs WHERE rowid=?1", rusqlite::params![id])
                .map_err(io_err)?;
            let removed = tx
                .execute("DELETE FROM records WHERE id=?1", rusqlite::params![id])
                .map_err(io_err)?;
            if removed != 1 {
                return Err(io_err("remove affected no record row"));
            }
            Ok(())
        })();
        if let Err(e) = step {
            drop(tx); // rollback: no partial record/vector/meta state
            return Err(e);
        }
        tx.commit().map_err(io_err)?;
        Ok(())
    }

    /// Apply a patch to one record atomically, preserving the public ID.
    ///
    /// `text`/`canonical`/`hash` are the effective new values (already
    /// validated, canonicalized, and hashed by the caller). `embedding` is
    /// `Some` only for real text changes and is written with the
    /// evidence-backed delete-plus-insert using the same explicit rowid
    /// (evidence items 5 and 8: re-insert visibility and rowid round-trip
    /// observed on the pinned stack), so the replacement vector row keeps
    /// the record ID. Metadata-only patches (`None`) leave `vecs` untouched.
    /// The `records` row is updated in place, so AUTOINCREMENT IDs stay
    /// stable and are never reused. One `Immediate` transaction covers the
    /// record, metadata, and optional vector writes; any pre-commit failure
    /// (including a `UNIQUE`-hash collision) rolls back with no partial
    /// state. A missing row is [`LibraryError::MissingDefinition`].
    pub(crate) fn apply_patch(
        &mut self,
        id: i64,
        text: &str,
        canonical: &str,
        hash: &str,
        embedding: Option<&[f32]>,
    ) -> Result<(), LibraryError> {
        let present: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM records WHERE id=?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .ok();
        if present != Some(id) {
            return Err(LibraryError::MissingDefinition { id: id.to_string() });
        }
        if let Some(emb) = embedding {
            if emb.len() != INGEST_DIMENSION as usize {
                return Err(io_err(format!("invalid embedding length {}", emb.len())));
            }
        }
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(io_err)?;
        let step: Result<(), LibraryError> = (|| {
            let updated = tx
                .execute(
                    "UPDATE records SET hash=?1, text=?2, metadata=?3 WHERE id=?4",
                    rusqlite::params![hash, text, canonical, id],
                )
                .map_err(io_err)?;
            if updated != 1 {
                return Err(io_err("patch affected no record row"));
            }
            tx.execute(
                "DELETE FROM meta_kv WHERE record_id=?1",
                rusqlite::params![id],
            )
            .map_err(io_err)?;
            let meta: serde_json::Value = serde_json::from_str(canonical).map_err(io_err)?;
            let obj = meta
                .as_object()
                .ok_or_else(|| io_err("metadata not object"))?;
            for (k, v) in obj {
                let vs = v.as_str().ok_or_else(|| io_err("metadata not flat"))?;
                tx.execute(
                    "INSERT INTO meta_kv(record_id,k,v) VALUES(?1,?2,?3)",
                    rusqlite::params![id, k, vs],
                )
                .map_err(io_err)?;
            }
            if let Some(emb) = embedding {
                tx.execute("DELETE FROM vecs WHERE rowid=?1", rusqlite::params![id])
                    .map_err(io_err)?;
                let mut bytes = Vec::with_capacity(emb.len() * 4);
                for f in emb.iter() {
                    bytes.extend_from_slice(&f.to_le_bytes());
                }
                tx.execute(
                    "INSERT INTO vecs(rowid, embedding) VALUES(?1, ?2)",
                    rusqlite::params![id, bytes],
                )
                .map_err(io_err)?;
            }
            Ok(())
        })();
        if let Err(e) = step {
            drop(tx); // rollback: no partial record/vector/meta state
            return Err(e);
        }
        tx.commit().map_err(io_err)?;
        Ok(())
    }

    /// Read one record's text, canonical metadata, and content hash by row id.
    ///
    /// Mutation-path counterpart to [`read_record`](Self::read_record) that
    /// additionally returns the stored hash so callers can detect identical
    /// patches and hash collisions without embedding. A missing row is the
    /// distinct [`LibraryError::MissingDefinition`].
    pub(crate) fn read_for_patch(&self, id: i64) -> Result<(String, String, String), LibraryError> {
        self.conn
            .query_row(
                "SELECT text, metadata, hash FROM records WHERE id=?1",
                rusqlite::params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    LibraryError::MissingDefinition { id: id.to_string() }
                }
                other => io_err(other),
            })
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
        // Process-local counter disambiguates parallel tests sharing a
        // timestamp (wall-clock nanos alone can collide within one process).
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let unique = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "infraware-vec-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            unique
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn count(conn: &rusqlite::Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn dir_snapshot(dir: &Path) -> (Vec<String>, Vec<(String, u64)>) {
        let mut names: Vec<String> = Vec::new();
        let mut bytes: Vec<(String, u64)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let len = std::fs::read(e.path()).map(|b| b.len() as u64).unwrap_or(0);
                names.push(name.clone());
                bytes.push((name, len));
            }
        }
        names.sort();
        bytes.sort();
        (names, bytes)
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

    /// Task 2.1: the ingest lane lazily creates the index at the exact
    /// owned path. `ingest::run` is the sole production caller of
    /// `SqliteStore::open`, so opening through it must materialize exactly
    /// `<root>/.infraware/vec-index.sqlite` from a fresh root.
    #[test]
    fn ingest_lane_open_creates_exact_index_path() {
        let root = tmp_root();
        let expected = root.join(".infraware").join("vec-index.sqlite");
        assert_eq!(vec_path(&root), expected);
        assert!(!expected.exists());
        let _store = SqliteStore::open(&root).unwrap();
        assert!(expected.is_file(), "ingest open must create {expected:?}");
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

    /// Task 3.6: storage-side dimension gate. Wrong-length embeddings are
    /// rejected before any write with no visible rows.
    #[test]
    fn insert_rejects_wrong_dimension_with_no_rows() {
        let root = tmp_root();
        let mut store = SqliteStore::open(&root).unwrap();
        for bad in [vec![0.1f32; 3], vec![0.1f32; INGEST_DIMENSION as usize + 1]] {
            assert!(store.insert("badh", "t", "{}", &bad).is_err());
        }
        assert_eq!(count(&store.conn, "SELECT count(*) FROM records"), 0);
        assert_eq!(count(&store.conn, "SELECT count(*) FROM vecs"), 0);
        assert_eq!(store.find_by_hash("badh").unwrap(), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_only_missing_creates_nothing() {
        let missing = tmp_root();
        assert!(!vec_path(&missing).exists());
        assert!(SqliteStore::open_read_only(&missing).is_err());
        assert!(!vec_path(&missing).exists());
        assert!(!missing.join(".infraware").exists());
        let _ = std::fs::remove_dir_all(&missing);
    }

    #[test]
    fn read_only_snapshot_preserves_original_and_reads_current() {
        let root = tmp_root();
        let emb = vec![0.5f32; INGEST_DIMENSION as usize];
        let id = {
            let mut w = SqliteStore::open(&root).unwrap();
            w.insert("qr1", "hello", r#"{"a":"1"}"#, &emb).unwrap()
        };
        let dir = root.join(".infraware");
        let (names_before, bytes_before) = dir_snapshot(&dir);
        let ro = SqliteStore::open_read_only(&root).unwrap();
        assert_eq!(ro.find_by_hash("qr1").unwrap(), Some(id));
        assert!(ro.conn.execute("CREATE TABLE ro_probe(x)", []).is_err());
        drop(ro);
        let (names_after, bytes_after) = dir_snapshot(&dir);
        assert_eq!(names_before, names_after, "original artifact set changed");
        assert_eq!(bytes_before, bytes_after, "original artifact bytes changed");
        assert!(
            !names_after
                .iter()
                .any(|n| n.ends_with("-shm") || n.ends_with("-journal")),
            "no new sidecars may persist in the original: {names_after:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_only_snapshot_sees_wal_with_live_writer() {
        // Committed-but-uncheckpointed WAL rows must survive the file copy.
        let root = tmp_root();
        let emb = vec![0.5f32; INGEST_DIMENSION as usize];
        let mut w = SqliteStore::open(&root).unwrap();
        let id = w.insert("wal1", "hello", r#"{"a":"1"}"#, &emb).unwrap();
        // Writer stays alive so frames remain uncheckpointed in -wal.
        let ro = SqliteStore::open_read_only(&root).unwrap();
        assert_eq!(ro.find_by_hash("wal1").unwrap(), Some(id));
        let n: i64 = ro
            .conn
            .query_row("SELECT count(*) FROM records", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
        drop(ro);
        drop(w);
        let _ = std::fs::remove_dir_all(&root);
    }

    // Task 4.2: read-only compatibility gate. Each test seeds via the
    // ingest opener, tampers through that writable handle (fixture setup
    // only), then validates exclusively through the `open_read_only`
    // snapshot connection, never the original.
    fn seeded_read_only() -> (PathBuf, SqliteStore) {
        let root = tmp_root();
        {
            let mut w = SqliteStore::open(&root).unwrap();
            let emb = vec![0.5f32; INGEST_DIMENSION as usize];
            w.insert("compat1", "hello", r#"{"a":"1"}"#, &emb).unwrap();
        }
        let ro = SqliteStore::open_read_only(&root).unwrap();
        (root, ro)
    }

    fn tampered_read_only(tamper: &[&str]) -> (PathBuf, SqliteStore) {
        let root = tmp_root();
        {
            let mut w = SqliteStore::open(&root).unwrap();
            let emb = vec![0.5f32; INGEST_DIMENSION as usize];
            w.insert("compat1", "hello", r#"{"a":"1"}"#, &emb).unwrap();
            for sql in tamper {
                w.conn.execute(sql, []).unwrap();
            }
        }
        let ro = SqliteStore::open_read_only(&root).unwrap();
        (root, ro)
    }

    fn tampered_meta_value(key: &str, bad: &str) -> (PathBuf, SqliteStore) {
        let root = tmp_root();
        {
            let mut w = SqliteStore::open(&root).unwrap();
            let emb = vec![0.5f32; INGEST_DIMENSION as usize];
            w.insert("compat1", "hello", r#"{"a":"1"}"#, &emb).unwrap();
            w.conn
                .execute(
                    "UPDATE model_meta SET value=?1 WHERE key=?2",
                    rusqlite::params![bad, key],
                )
                .unwrap();
        }
        let ro = SqliteStore::open_read_only(&root).unwrap();
        (root, ro)
    }

    /// Gate helper mirroring the future query order: validate first, embed
    /// only on success. The fake embedder counts calls to prove validation
    /// precedes any later embed call.
    fn embed_if_compatible(store: &SqliteStore, embed_calls: &mut u32) -> Result<(), LibraryError> {
        store.verify_query_compatible()?;
        *embed_calls += 1;
        Ok(())
    }

    #[test]
    fn compat_passes_on_fresh_snapshot() {
        let (root, ro) = seeded_read_only();
        assert!(ro.verify_query_compatible().is_ok());
        let mut calls = 0;
        embed_if_compatible(&ro, &mut calls).unwrap();
        assert_eq!(calls, 1);
        drop(ro);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn compat_rejects_missing_vecs_schema() {
        let (root, ro) = tampered_read_only(&["DROP TABLE vecs"]);
        assert!(ro.verify_query_compatible().is_err());
        let mut calls = 0;
        assert!(embed_if_compatible(&ro, &mut calls).is_err());
        assert_eq!(calls, 0, "embed must not run after failed validation");
        drop(ro);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn compat_rejects_wrong_vecs_shape() {
        let (root, ro) = tampered_read_only(&[
            "DROP TABLE vecs",
            "CREATE VIRTUAL TABLE vecs USING vec0(embedding float[768] distance_metric=cosine)",
        ]);
        assert!(ro.verify_query_compatible().is_err());
        let mut calls = 0;
        assert!(embed_if_compatible(&ro, &mut calls).is_err());
        assert_eq!(calls, 0);
        drop(ro);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn compat_rejects_missing_records_table() {
        let (root, ro) = tampered_read_only(&["DROP TABLE meta_kv", "DROP TABLE records"]);
        assert!(ro.verify_query_compatible().is_err());
        let mut calls = 0;
        assert!(embed_if_compatible(&ro, &mut calls).is_err());
        assert_eq!(calls, 0);
        drop(ro);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn compat_rejects_missing_meta_kv_table() {
        let (root, ro) = tampered_read_only(&["DROP TABLE meta_kv"]);
        assert!(ro.verify_query_compatible().is_err());
        let mut calls = 0;
        assert!(embed_if_compatible(&ro, &mut calls).is_err());
        assert_eq!(calls, 0);
        drop(ro);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn compat_rejects_missing_model_meta_table() {
        let (root, ro) = tampered_read_only(&["DROP TABLE model_meta"]);
        assert!(ro.verify_query_compatible().is_err());
        let mut calls = 0;
        assert!(embed_if_compatible(&ro, &mut calls).is_err());
        assert_eq!(calls, 0);
        drop(ro);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn compat_rejects_missing_meta_index() {
        let (root, ro) = tampered_read_only(&["DROP INDEX idx_meta_kv_kv"]);
        assert!(ro.verify_query_compatible().is_err());
        let mut calls = 0;
        assert!(embed_if_compatible(&ro, &mut calls).is_err());
        assert_eq!(calls, 0);
        drop(ro);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn compat_rejects_each_bad_model_meta_value() {
        for (key, bad) in [
            ("schema_version", "2"),
            ("model", "other/model"),
            ("revision", "deadbeef"),
            ("dimension", "768"),
            ("metric", "euclidean"),
            ("normalized", "0"),
        ] {
            let (root, ro) = tampered_meta_value(key, bad);
            assert!(
                ro.verify_query_compatible().is_err(),
                "key {key} with bad value must fail"
            );
            let mut calls = 0;
            assert!(embed_if_compatible(&ro, &mut calls).is_err());
            assert_eq!(calls, 0, "embed must not run when {key} mismatches");
            drop(ro);
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// Byte-exact fingerprint of the four query-relevant artifacts for task 4.4:
    /// main DB plus `-wal`/`-shm`/`-journal` siblings. `None` means absent;
    /// `Some(bytes)` is the full file content. Compares presence and bytes.
    fn artifact_fingerprint(root: &Path) -> Vec<(String, Option<Vec<u8>>)> {
        let main = vec_path(root);
        let base = main.as_os_str().to_owned();
        let mut out = Vec::new();
        out.push(("vec-index.sqlite".to_string(), std::fs::read(&main).ok()));
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut s = base.clone();
            s.push(suffix);
            let p = PathBuf::from(s);
            let label = format!("vec-index.sqlite{suffix}");
            out.push((label, std::fs::read(&p).ok()));
        }
        out
    }

    /// Task 4.4: valid read-only path leaves all four artifacts byte-identical
    /// and cleans up the ephemeral snapshot directory.
    #[test]
    fn read_only_valid_path_preserves_all_artifacts_and_cleans_snapshot() {
        let root = tmp_root();
        let mut w = SqliteStore::open(&root).unwrap();
        let emb = vec![0.5f32; INGEST_DIMENSION as usize];
        let id = w.insert("fp1", "hello", r#"{"a":"1"}"#, &emb).unwrap();
        // Keep the writer alive so any WAL/SHM state is present during copy.
        let before = artifact_fingerprint(&root);
        assert!(before[0].1.is_some(), "main DB must exist before query");
        let ro = SqliteStore::open_read_only(&root).unwrap();
        let snap = ro.snapshot_path().expect("snapshot must exist while open");
        assert!(
            snap.is_dir(),
            "snapshot dir must exist while store is alive"
        );
        // Exercise the actual read-only constructor + compatibility gate.
        assert_eq!(ro.find_by_hash("fp1").unwrap(), Some(id));
        assert!(ro.verify_query_compatible().is_ok());
        assert!(ro.conn.execute("CREATE TABLE ro_fp_probe(x)", []).is_err());
        drop(ro);
        assert!(!snap.exists(), "snapshot dir must be removed after drop");
        let after = artifact_fingerprint(&root);
        assert_eq!(before, after, "original artifacts must be byte-identical");
        drop(w);
        // Closing the writer must not leave query sidecars behind either.
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Task 4.4: incompatible metadata fails safely without mutating the original
    /// and still cleans up the snapshot.
    #[test]
    fn read_only_incompatible_fails_without_mutating_original() {
        let root = tmp_root();
        {
            let mut w = SqliteStore::open(&root).unwrap();
            let emb = vec![0.5f32; INGEST_DIMENSION as usize];
            w.insert("fp2", "hello", r#"{"a":"1"}"#, &emb).unwrap();
            w.conn
                .execute(
                    "UPDATE model_meta SET value='768' WHERE key='dimension'",
                    [],
                )
                .unwrap();
        }
        let before = artifact_fingerprint(&root);
        let ro = SqliteStore::open_read_only(&root).unwrap();
        let snap = ro.snapshot_path().expect("snapshot must exist while open");
        assert!(ro.verify_query_compatible().is_err());
        let mut calls = 0;
        assert!(embed_if_compatible(&ro, &mut calls).is_err());
        assert_eq!(calls, 0);
        drop(ro);
        assert!(
            !snap.exists(),
            "snapshot dir must be removed after failed gate"
        );
        let after = artifact_fingerprint(&root);
        assert_eq!(before, after, "incompatible gate must not mutate original");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Task 4.4: missing index creates no `.infraware` artifacts.
    /// No snapshot is materialized: `open_read_only` returns before any temp
    /// copy (early existence check), so there is nothing to leak or clean up.
    #[test]
    fn read_only_missing_creates_no_artifacts_or_snapshot() {
        let missing = tmp_root();
        assert!(SqliteStore::open_read_only(&missing).is_err());
        assert!(!missing.join(".infraware").exists());
        assert!(!vec_path(&missing).exists());
        let _ = std::fs::remove_dir_all(&missing);
    }

    #[test]
    fn compat_rejects_missing_model_meta_row() {
        let (root, ro) = tampered_read_only(&["DELETE FROM model_meta WHERE key='revision'"]);
        assert!(ro.verify_query_compatible().is_err());
        let mut calls = 0;
        assert!(embed_if_compatible(&ro, &mut calls).is_err());
        assert_eq!(calls, 0);
        drop(ro);
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- Lane 2 (tasks 6.2-6.4): remove/patch against Spike 4 evidence ----
    //
    // Every statement below is evidence-backed only: `DELETE FROM vecs
    // WHERE rowid = ?` legality (probe item 4), delete+reinsert visibility
    // of an explicit rowid (item 5), delete-path rollback (item 6), stable
    // shadow tables at small scale (item 7), and explicit-rowid round-trip
    // (item 8). No invented DDL, KNN, or chunk API appears here.

    fn seed_two(root: &Path) -> (i64, i64) {
        let mut w = SqliteStore::open(root).unwrap();
        let emb = vec![0.5f32; INGEST_DIMENSION as usize];
        let a = w.insert("rm-a", "alpha", r#"{"g":"1"}"#, &emb).unwrap();
        let b = w.insert("rm-b", "beta", r#"{"g":"2"}"#, &emb).unwrap();
        (a, b)
    }

    fn table_counts(conn: &rusqlite::Connection) -> (i64, i64, i64) {
        (
            count(conn, "SELECT count(*) FROM records"),
            count(conn, "SELECT count(*) FROM meta_kv"),
            count(conn, "SELECT count(*) FROM vecs"),
        )
    }

    /// Shadow-table set for the vec0 index (probe item 7 vocabulary).
    /// Sorted names of `vecs` plus every `vecs_%` shadow table.
    fn shadow_set(conn: &rusqlite::Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE name='vecs' OR name LIKE 'vecs\\_%' ESCAPE '\\' ORDER BY name")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<Vec<String>, _>>()
            .unwrap()
    }

    fn embedding_bytes(conn: &rusqlite::Connection, id: i64) -> Vec<u8> {
        conn.query_row(
            "SELECT embedding FROM vecs WHERE rowid=?1",
            rusqlite::params![id],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// Task 6.2: remove deletes the record, metadata, and vector rows
    /// atomically; repeating the removal is record-not-found.
    #[test]
    fn remove_deletes_all_three_tables_atomically() {
        let root = tmp_root();
        let (a, b) = seed_two(&root);
        let mut store = SqliteStore::open_existing_rw(&root).unwrap();
        assert_eq!(table_counts(&store.conn), (2, 2, 2));
        store.remove_record(a).unwrap();
        assert_eq!(table_counts(&store.conn), (1, 1, 1));
        // The survivor is fully linked; the removed id reads back nothing.
        assert!(store.find_by_hash("rm-b").unwrap() == Some(b));
        assert!(store.find_by_hash("rm-a").unwrap().is_none());
        assert!(store.read_for_patch(b).is_ok());
        // Repeat removal is the distinct record-not-found error.
        assert!(matches!(
            store.remove_record(a),
            Err(LibraryError::MissingDefinition { .. })
        ));
        assert_eq!(table_counts(&store.conn), (1, 1, 1));
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Task 6.2: the remove statements roll back together (probe item 6).
    ///
    /// Mirrors the executed probe: one Immediate transaction performs the
    /// three production deletes, a forced failing statement errors, and
    /// rollback restores every row with no partial state.
    #[test]
    fn remove_statements_roll_back_together_on_failure() {
        let root = tmp_root();
        let (a, _) = seed_two(&root);
        let mut store = SqliteStore::open_existing_rw(&root).unwrap();
        let before = table_counts(&store.conn);
        let tx = store
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        tx.execute(
            "DELETE FROM meta_kv WHERE record_id=?1",
            rusqlite::params![a],
        )
        .unwrap();
        tx.execute("DELETE FROM vecs WHERE rowid=?1", rusqlite::params![a])
            .unwrap();
        tx.execute("DELETE FROM records WHERE id=?1", rusqlite::params![a])
            .unwrap();
        // Forced failure on the same transaction, as in the probe.
        assert!(
            tx.execute("DELETE FROM vecs_nonexistent", [])
                .unwrap_err()
                .to_string()
                .contains("no such table")
        );
        tx.rollback().unwrap();
        assert_eq!(table_counts(&store.conn), before);
        assert!(store.find_by_hash("rm-a").unwrap() == Some(a));
        assert_eq!(
            embedding_bytes(&store.conn, a).len(),
            INGEST_DIMENSION as usize * 4
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Task 6.2: remove on missing storage creates nothing and fails
    /// operationally (never record-not-found, never usage).
    #[test]
    fn remove_open_missing_storage_creates_nothing() {
        let missing = tmp_root();
        assert!(SqliteStore::open_existing_rw(&missing).is_err());
        assert!(!missing.join(".infraware").exists());
        assert!(!vec_path(&missing).exists());
        let _ = std::fs::remove_dir_all(&missing);
    }

    /// Tasks 6.2/6.4: AUTOINCREMENT IDs are not reused after deletion and
    /// the shadow-table set stays consistent across remove (probe item 7:
    /// stable shadows at small scale, no orphan growth observed).
    #[test]
    fn remove_preserves_id_uniqueness_and_shadow_state() {
        let root = tmp_root();
        let (a, _) = seed_two(&root);
        let mut store = SqliteStore::open_existing_rw(&root).unwrap();
        let shadows_before = shadow_set(&store.conn);
        assert!(shadows_before.contains(&"vecs".to_string()));
        assert!(shadows_before.iter().any(|n| n.starts_with("vecs_")));
        store.remove_record(a).unwrap();
        let emb = vec![0.5f32; INGEST_DIMENSION as usize];
        let c = store.insert("rm-c", "gamma", r#"{"g":"3"}"#, &emb).unwrap();
        assert!(c > a, "deleted id {a} must not be reused (got {c})");
        // Live-row linkage and shadow consistency hold after remove+insert.
        assert_eq!(table_counts(&store.conn), (2, 2, 2));
        assert_eq!(shadow_set(&store.conn), shadows_before);
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Task 6.3: metadata-only patch replaces the side-table rows and record
    /// fields while leaving the stored vector bytes untouched (zero model
    /// work at the orchestration layer is covered in `patch.rs` tests).
    #[test]
    fn patch_metadata_only_preserves_vector_and_rowid() {
        let root = tmp_root();
        let (a, b) = seed_two(&root);
        let mut store = SqliteStore::open_existing_rw(&root).unwrap();
        let vec_before = embedding_bytes(&store.conn, a);
        let new_canonical = r#"{"g":"9","h":"x"}"#;
        let new_hash = crate::metadata::compute_identity(new_canonical, "alpha");
        store
            .apply_patch(a, "alpha", new_canonical, &new_hash, None)
            .unwrap();
        // Vector bytes and rowid linkage unchanged; metadata replaced wholesale.
        assert_eq!(embedding_bytes(&store.conn, a), vec_before);
        assert_eq!(table_counts(&store.conn), (2, 3, 2));
        let v: String = store
            .conn
            .query_row(
                "SELECT v FROM meta_kv WHERE record_id=?1 AND k='g'",
                rusqlite::params![a],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, "9");
        let (text, canonical, hash) = store.read_for_patch(a).unwrap();
        assert_eq!(
            (text.as_str(), canonical.as_str(), hash.as_str()),
            ("alpha", new_canonical, new_hash.as_str())
        );
        // The other record is untouched.
        assert!(store.find_by_hash("rm-b").unwrap() == Some(b));
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Task 6.3/6.4: text-change patch re-embeds via evidence-backed
    /// delete-plus-insert with the SAME explicit rowid (probe items 5+8),
    /// keeping the public ID stable while the vector bytes change.
    #[test]
    fn patch_text_change_replaces_vector_with_same_rowid() {
        let root = tmp_root();
        let (a, _) = seed_two(&root);
        let mut store = SqliteStore::open_existing_rw(&root).unwrap();
        let vec_before = embedding_bytes(&store.conn, a);
        let mut new_emb = vec![0.1f32; INGEST_DIMENSION as usize];
        new_emb[0] = 0.9;
        let new_hash = crate::metadata::compute_identity(r#"{"g":"1"}"#, "alpha v2");
        store
            .apply_patch(a, "alpha v2", r#"{"g":"1"}"#, &new_hash, Some(&new_emb))
            .unwrap();
        // Same rowid, new bytes, identical table footprint.
        let vec_after = embedding_bytes(&store.conn, a);
        assert_ne!(vec_after, vec_before);
        assert_eq!(vec_after.len(), INGEST_DIMENSION as usize * 4);
        assert_eq!(table_counts(&store.conn), (2, 2, 2));
        let nvec: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM vecs WHERE rowid=?1",
                rusqlite::params![a],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nvec, 1);
        assert!(store.find_by_hash(&new_hash).unwrap() == Some(a));
        assert!(store.find_by_hash("rm-a").unwrap().is_none());
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Task 6.3: a patch whose hash collides with another live record fails
    /// inside the transaction (UNIQUE) with no partial state on either row.
    #[test]
    fn patch_hash_collision_rolls_back_with_both_rows_intact() {
        let root = tmp_root();
        let (a, b) = seed_two(&root);
        let mut store = SqliteStore::open_existing_rw(&root).unwrap();
        let before_a = store.read_for_patch(a).unwrap();
        let before_b = store.read_for_patch(b).unwrap();
        let vec_a_before = embedding_bytes(&store.conn, a);
        // Force B onto A's hash through the store seam (orchestration would
        // reject this before embedding; here it proves tx atomicity).
        let colliding_hash = before_a.2.clone();
        let emb = vec![0.5f32; INGEST_DIMENSION as usize];
        assert!(
            store
                .apply_patch(b, "alpha", r#"{"g":"1"}"#, &colliding_hash, Some(&emb))
                .is_err()
        );
        assert_eq!(store.read_for_patch(a).unwrap(), before_a);
        assert_eq!(store.read_for_patch(b).unwrap(), before_b);
        assert_eq!(embedding_bytes(&store.conn, a), vec_a_before);
        assert_eq!(table_counts(&store.conn), (2, 2, 2));
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Task 6.3: patch on a missing row is record-not-found with no writes.
    #[test]
    fn patch_missing_row_is_not_found_with_no_side_effects() {
        let root = tmp_root();
        seed_two(&root);
        let mut store = SqliteStore::open_existing_rw(&root).unwrap();
        let before = table_counts(&store.conn);
        let emb = vec![0.5f32; INGEST_DIMENSION as usize];
        assert!(matches!(
            store.apply_patch(999, "x", "{}", "deadbeef", Some(&emb)),
            Err(LibraryError::MissingDefinition { .. })
        ));
        assert_eq!(table_counts(&store.conn), before);
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }
}
