//! Read-only query execution seam (tasks 4.3 storage gate).
//!
//! This module is the intended dispatch target for [`Command::Query`](crate::Command).
//! Full behavior — validation/output (tasks 3.x done), read-only storage (task
//! 4.3 done for open/verify/safe-mapping), cached model/embedding (tasks
//! 5.x), and reader/ranking (tasks 6.x) — lands in later tasks. [`run`]
//! opens the index through the read-only snapshot constructor, verifies
//! schema plus all six `model_meta` values, then returns a generic
//! operational error until retrieval lands, so the frozen stream/status
//! contract holds: empty stdout, one generic `error:` stderr line, status 1
//! via [`LibraryError::is_usage`]..
//!
//! Task 3.3 provides only the query-layer serde representation and compact
//! rendering for the exact success envelope
//! `{query,filter,max_distance:0.15,hits}` with hit fields exactly
//! `{id,score,distance,metadata,text}`. No thresholding, ranking, storage,
//! model, or retrieval logic lives here.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::dispatch::DispatchOutput;
use crate::error::LibraryError;

/// Fixed inclusive distance threshold echoed in every success envelope.
pub(crate) const QUERY_MAX_DISTANCE: f64 = 0.15;

/// Single ranked hit in the success envelope.
///
/// Field declaration order is the serialization order:
/// `id`, `score`, `distance`, `metadata`, `text`, `link`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct QueryHit {
    pub(crate) id: i64,
    pub(crate) score: f64,
    pub(crate) distance: f64,
    pub(crate) metadata: BTreeMap<String, String>,
    pub(crate) text: String,
    /// Link only present on the top hit (hit index 0). Derived from the
    /// top result's ID and its stored relation: `{fact_id, query_id,
    /// created_at}`. Omitted for hits 1..n and for zero-hit results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) link: Option<Link>,
}

/// The link object carried on the top hit only.
///
/// `{fact_id, query_id, created_at}` — fact_id is the record id of the
/// top result, query_id identifies the associated query, and created_at
/// is the fact's creation timestamp. Omitted when there are no hits.
// Dormant until tasks 3.2/5.x: top-hit link attachment constructs this.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Link {
    pub(crate) fact_id: i64,
    pub(crate) query_id: i64,
    pub(crate) created_at: i64,
}

/// Success envelope for `query`.
///
/// Field declaration order is the serialization order:
/// `query`, `filter`, `max_distance`, `hits`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct QueryEnvelope {
    pub(crate) query: String,
    pub(crate) filter: BTreeMap<String, String>,
    pub(crate) max_distance: f64,
    pub(crate) hits: Vec<QueryHit>,
}

/// Render the exact compact success JSON.
///
/// - Preserves `query` text bytes exactly (no trim/normalize).
/// - `filter` and per-hit `metadata` serialize via `BTreeMap` byte order.
/// - Compact form only (`serde_json::to_string`); empty `hits` renders `[]`.
/// - No extra fields or alternate output modes.
///
/// Link is included on the top hit only (hits[0]); hits[1..] have
/// `link: null` omitted by `skip_serializing_if`. When `hits` is empty
/// the envelope has no `link` field.
pub(crate) fn render_envelope(
    query: &str,
    filter: &BTreeMap<String, String>,
    hits: &[QueryHit],
) -> String {
    let envelope = QueryEnvelope {
        query: query.to_string(),
        filter: filter.clone(),
        max_distance: QUERY_MAX_DISTANCE,
        hits: hits.to_vec(),
    };
    // Serialization of owned strings/floats/maps cannot fail; panic would
    // indicate a programming error, but fall back to a minimal envelope
    // rather than introducing a new error path at the query layer.
    serde_json::to_string(&envelope).expect("query envelope serialization must succeed")
}

/// Candidate row handed to the ranking seam.
///
/// Rows are produced by the later reader seam (task 6.x) already joined
/// with exact metadata filtering; [`rank_candidates`] applies only the
/// inclusive distance threshold, score math, and deterministic ordering.
/// Pure data: no SQLite, model, filesystem, or network access.
// Legacy parallel threshold/order path; production uses ranked_qualifying_ids +
// assemble_hits. Retained until the accepted post-2.x cleanup removes it.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QueryCandidate {
    pub(crate) id: i64,
    pub(crate) distance: f64,
    pub(crate) metadata: BTreeMap<String, String>,
    pub(crate) text: String,
}

/// Keep every candidate with `distance <= QUERY_MAX_DISTANCE` (inclusive),
/// compute `score` exactly as `1.0 - distance`, and sort by distance
/// ascending then `id` ascending.
///
/// - Non-finite or above-threshold distances are rejected by the `<=`
///   comparison (NaN never satisfies `<=`).
/// - Ordering uses total float ordering so equal distances fall back to
///   integer `id` order deterministically.
/// - Pure: borrows the slice, returns owned hits, performs no I/O.
// Legacy parallel threshold/order path (see QueryCandidate). Retained until the
// accepted post-2.x cleanup removes it.
#[allow(dead_code)]
pub(crate) fn rank_candidates(candidates: &[QueryCandidate]) -> Vec<QueryHit> {
    let mut hits: Vec<QueryHit> = candidates
        .iter()
        .filter(|c| c.distance <= QUERY_MAX_DISTANCE)
        .map(|c| QueryHit {
            id: c.id,
            score: 1.0 - c.distance,
            distance: c.distance,
            metadata: c.metadata.clone(),
            text: c.text.clone(),
            link: None,
        })
        .collect();
    hits.sort_by(|a, b| {
        a.distance
            .total_cmp(&b.distance)
            .then_with(|| a.id.cmp(&b.id))
    });
    hits
}

/// Deep retrieval seam (task 6.1) with exact metadata filtering (task 6.2).
///
/// Proven spike strategy: read rows via a filtered fullscan on the
/// read-only snapshot (`SELECT rowid, embedding FROM vecs` plus one
/// parameterized `EXISTS` clause per canonical filter pair, empty filter
/// means no `WHERE` clause), decode each stored 384-f32 blob, and rescore
/// exactly in Rust with [`cosine_distance`]. No engine-side bound,
/// distance function, or row cap is used here. Task 6.3 fetches record
/// text/metadata for survivors after ranking plus final output.
/// Statements are never held across embedding: `run_with_model_embed`
/// embeds first, then scans on the already-open snapshot connection.
///
/// [`EmbeddingReader`] is the small seam: fakes prove ranking logic while
/// the [`SqliteStore`](crate::vec_store::SqliteStore) adapter is the
/// production reader. [`retrieve_scored`] is the thin adapter entry point
/// shared by both.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ScoredRow {
    pub(crate) id: i64,
    pub(crate) distance: f64,
}

/// Small reader seam for the deep query path (tasks 6.1-6.2).
///
/// The canonical `filter` is applied against the `meta_kv` side table
/// before any rescore/ranking work: empty means all rows, non-empty means
/// exact string-equality AND over every key/value pair. Production adapter:
/// [`SqliteStore`](crate::vec_store::SqliteStore) filtered fullscan read.
/// Fake adapters return canned `(rowid, embedding, metadata)` rows and
/// apply the same AND semantics in Rust so rescore and later ranking logic
/// stay testable without a DB or model.
pub(crate) trait EmbeddingReader {
    fn read_embeddings(
        &self,
        filter: &BTreeMap<String, String>,
    ) -> Result<Vec<(i64, Vec<f32>)>, LibraryError>;
}

impl EmbeddingReader for crate::vec_store::SqliteStore {
    fn read_embeddings(
        &self,
        filter: &BTreeMap<String, String>,
    ) -> Result<Vec<(i64, Vec<f32>)>, LibraryError> {
        self.scan_filtered_embeddings(filter)
    }
}

/// Exact cosine distance `1 - dot(a,b)/(|a|*|b|)` in f64.
///
/// Zero-norm inputs map to distance 1.0 (orthogonal fallback) so rescore
/// never divides by zero; stored and query embeddings are normalized, so
/// the common path is `1 - dot`.
pub(crate) fn cosine_distance(a: &[f32], b: &[f32]) -> f64 {
    debug_assert_eq!(a.len(), b.len());
    let mut dot: f64 = 0.0;
    let mut na: f64 = 0.0;
    let mut nb: f64 = 0.0;
    for (x, y) in a.iter().zip(b.iter()) {
        let x = *x as f64;
        let y = *y as f64;
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = na.sqrt() * nb.sqrt();
    if !denom.is_finite() || denom <= 0.0 {
        return 1.0;
    }
    1.0 - dot / denom
}

/// Task 2.3: entity-type tag for one embedding operand.
///
/// The store holds fact vectors only; the query embedding is produced per
/// call by the model. Tagging each operand at the seam makes the
/// same-type rule (`facts-to-facts`, `queries-to-facts`, never
/// `query-vs-query`) checkable instead of merely conventional.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmbeddingKind {
    Query,
    Fact,
}

/// Task 2.3: a borrowed embedding slice tagged with its entity type.
///
/// Borrowed (`&[f32]`, never owned/`Clone`) per the lending convention:
/// the tag travels with the borrow and no vector bytes are copied.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TypedEmbedding<'a> {
    kind: EmbeddingKind,
    values: &'a [f32],
}

impl<'a> TypedEmbedding<'a> {
    pub(crate) fn query(values: &'a [f32]) -> Self {
        Self {
            kind: EmbeddingKind::Query,
            values,
        }
    }

    pub(crate) fn fact(values: &'a [f32]) -> Self {
        Self {
            kind: EmbeddingKind::Fact,
            values,
        }
    }
}

/// Task 2.3: same-type-gated cosine distance.
///
/// `Query`-vs-`Fact` (either order) and `Fact`-vs-`Fact` compute the exact
/// [`cosine_distance`]; `Query`-vs-`Query` is rejected as an operational
/// error (never usage: a cross-type comparison is an internal invariant
/// violation, so it must map to status 1, never status 2). The message is
/// a static token with no paths, SQL, or vector contents.
pub(crate) fn typed_cosine_distance(
    a: &TypedEmbedding<'_>,
    b: &TypedEmbedding<'_>,
) -> Result<f64, LibraryError> {
    match (a.kind, b.kind) {
        (EmbeddingKind::Query, EmbeddingKind::Query) => Err(LibraryError::Io {
            message: "cross-type embedding comparison rejected".to_string(),
        }),
        _ => Ok(cosine_distance(a.values, b.values)),
    }
}

/// Rescore every scanned row against the query embedding in Rust.
///
/// Returns all rows as [`ScoredRow`] sorted by distance ascending then id
/// ascending (same deterministic order as the final ranking seam). No
/// filtering, thresholding, or output here: tasks 6.2/6.3 own those.
///
/// Task 2.3: every comparison runs through [`typed_cosine_distance`] with
/// the live query tagged `Query` and each scanned row tagged `Fact`, so a
/// query-vs-query comparison cannot reach the math by construction (rows
/// come only from [`EmbeddingReader`], which reads stored fact vectors).
pub(crate) fn rescore(query_embedding: &[f32], rows: &[(i64, Vec<f32>)]) -> Vec<ScoredRow> {
    let query = TypedEmbedding::query(query_embedding);
    let mut out: Vec<ScoredRow> = rows
        .iter()
        .map(|(id, emb)| ScoredRow {
            id: *id,
            distance: typed_cosine_distance(&query, &TypedEmbedding::fact(emb))
                // Infallible by construction: the only pairing built here is
                // query-vs-fact, which the gate always accepts.
                .expect("query-vs-fact pairing is always valid"),
        })
        .collect();
    out.sort_by(|a, b| {
        a.distance
            .total_cmp(&b.distance)
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// Thin adapter entry point: filtered scan through the reader seam, then
/// exact-Rust rescore over the filtered candidate set only. The filter is
/// enforced before rescore (inside the reader), so unfiltered rows never
/// reach ranking. Pure wiring; no threshold/output beyond this.
pub(crate) fn retrieve_scored(
    reader: &impl EmbeddingReader,
    query_embedding: &[f32],
    filter: &BTreeMap<String, String>,
) -> Result<Vec<ScoredRow>, LibraryError> {
    let rows = reader.read_embeddings(filter)?;
    Ok(rescore(query_embedding, &rows))
}

/// Post-rank record seam: text plus canonical metadata for one row id.
///
/// Production adapter is [`SqliteStore`](crate::vec_store::SqliteStore)::`read_record`;
/// fakes return canned pairs and record the call order so post-rank ordering
/// stays observable without a DB. Static dispatch (`impl` closure / generic
/// reader) keeps the seam testable with no heap indirection.
#[allow(dead_code)]
pub(crate) trait RecordReader {
    fn read_record(&self, id: i64) -> Result<(BTreeMap<String, String>, String), LibraryError>;
}

#[allow(dead_code)]
impl RecordReader for crate::vec_store::SqliteStore {
    fn read_record(&self, id: i64) -> Result<(BTreeMap<String, String>, String), LibraryError> {
        Self::read_record(self, id)
    }
}

/// Ranked qualifying ids in deterministic output order.
///
/// Pure: filters `distance <= QUERY_MAX_DISTANCE` (inclusive; NaN never
/// satisfies `<=`) and sorts by distance ascending then id ascending. The
/// returned order is exactly the post-rank read order and the envelope
/// order, so tests can assert it without touching SQLite.
pub(crate) fn ranked_qualifying_ids(scored: &[ScoredRow]) -> Vec<(i64, f64)> {
    let mut kept: Vec<(i64, f64)> = scored
        .iter()
        .filter(|r| r.distance <= QUERY_MAX_DISTANCE)
        .map(|r| (r.id, r.distance))
        .collect();
    kept.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    kept
}

/// Assemble ranked hits by reading text/metadata only for qualifiers.
///
/// Pure wiring over an injected `load_one` closure: `scored` is already the
/// filtered-fullscan rescore (filter applied before rescore), so this stage
/// only thresholds, orders, then reads each qualifying id exactly once in
/// ranked order. `score` is exactly `1.0 - distance`. A missing row or
/// malformed record from `load_one` propagates for the caller to map to
/// the safe operational error. No SQLite, model, or network access here.
pub(crate) fn assemble_hits(
    scored: &[ScoredRow],
    load_one: impl Fn(i64) -> Result<(BTreeMap<String, String>, String), LibraryError>,
) -> Result<Vec<QueryHit>, LibraryError> {
    let mut hits = Vec::new();
    for (id, distance) in ranked_qualifying_ids(scored) {
        let (metadata, text) = load_one(id)?;
        hits.push(QueryHit {
            id,
            score: 1.0 - distance,
            distance,
            metadata,
            text,
            link: None,
        });
    }
    Ok(hits)
}

/// Render the ranked success output for the exact compact envelope.
///
/// Thin success constructor shared by production and tests: assembles hits
/// through `load_one`, then renders via [`render_envelope`] preserving the
/// byte-exact `query` text and canonical `filter` bytes. Empty qualifying
/// set yields `hits: []` with status 0 (caller returns `Ok`).
///
/// Retained as a compatibility/test seam for the pre-persistence rendering
/// path; production dispatch now performs persistence before rendering.
#[allow(dead_code)]
pub(crate) fn render_success(
    query_text: &str,
    filter: &BTreeMap<String, String>,
    scored: &[ScoredRow],
    load_one: impl Fn(i64) -> Result<(BTreeMap<String, String>, String), LibraryError>,
) -> Result<DispatchOutput, LibraryError> {
    let hits = assemble_hits(scored, load_one)?;
    let body = render_envelope(query_text, filter, &hits);
    Ok(DispatchOutput {
        format: crate::format::OutputFormat::Json,
        body,
    })
}

/// Generic operational failure surfaced for every query storage error.
///
/// Deliberately fixed and detail-free: no filesystem paths, SQL, vectors,
/// model/cache details, or internal error text. `run` renders it as
/// `error: i/o error: query failed` (one stderr line, empty stdout).
pub(crate) const QUERY_OPERATIONAL_MESSAGE: &str = "query failed";

fn operational_err() -> LibraryError {
    LibraryError::Io {
        message: QUERY_OPERATIONAL_MESSAGE.to_string(),
    }
}

/// Ordered query setup seam (task 5.2).
///
/// Guarantees the exact pre-retrieval order on the production path:
/// 1. open the read-only snapshot index,
/// 2. verify schema plus all six `model_meta` values,
/// 3. validate the cached-only model files (no fetcher, no network),
/// 4. embed the byte-exact query text.
///
/// `resolve` must be the cached-only accessor (never a downloader) and
/// `embed` receives the original `text` bytes unchanged (no trim/normalize).
/// Every stage failure short-circuits to the single generic operational
/// error; underlying details are discarded, never propagated. Order is
/// open -> verify -> cached model -> embed -> fullscan read (statement
/// created only after embedding, never held across it). Retrieval output
/// (filter/rank/render) lands in tasks 6.2-6.3, so the post-scan path
/// still returns the same generic operational error.
pub(crate) fn run_with_model_embed(
    root: &Path,
    text: &str,
    filter: &BTreeMap<String, String>,
    resolve: impl FnOnce() -> Result<crate::model_cache::ModelPaths, LibraryError>,
    embed: impl FnOnce(&crate::model_cache::ModelPaths, &str) -> Result<Vec<f32>, LibraryError>,
) -> Result<DispatchOutput, LibraryError> {
    let store = match crate::vec_store::SqliteStore::open_read_only(root) {
        Ok(store) => store,
        Err(_) => return Err(operational_err()),
    };
    if store.verify_query_compatible().is_err() {
        return Err(operational_err());
    }
    let paths = match resolve() {
        Ok(paths) => paths,
        Err(_) => return Err(operational_err()),
    };
    let query_embedding = match embed(&paths, text) {
        Ok(embedding) => embedding,
        Err(_) => return Err(operational_err()),
    };
    // Deep reader wiring (tasks 6.1-6.3): filtered fullscan production
    // read plus exact Rust rescore through the seam. The canonical filter
    // is applied against `meta_kv` before rescore, so only matching rows
    // reach ranking. Post-rank record reads run only for qualifying ids in
    // ranked order, then the exact compact envelope renders. Every failure
    // maps to the single generic operational error.
    let scored = match retrieve_scored(&store, &query_embedding, filter) {
        Ok(scored) => scored,
        Err(_) => return Err(operational_err()),
    };
    let mut hits = match assemble_hits(&scored, |id| store.read_record(id)) {
        Ok(hits) => hits,
        Err(_) => return Err(operational_err()),
    };
    if hits.is_empty() {
        let body = render_envelope(text, filter, &hits);
        return Ok(DispatchOutput {
            format: crate::format::OutputFormat::Json,
            body,
        });
    }

    // Retrieval is complete before opening the separate write connection.
    // The snapshot and its SQL borrows are dropped before the RW transaction.
    let top_fact_id = hits[0].id;
    let canonical_filter =
        serde_json::to_string(filter).expect("BTreeMap JSON serialization must succeed");
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| operational_err())
        .and_then(|d| i64::try_from(d.as_millis()).map_err(|_| operational_err()))?;
    drop(store);
    let mut rw = match crate::vec_store::SqliteStore::open_existing_rw(root) {
        Ok(store) => store,
        Err(_) => return Err(operational_err()),
    };
    let link = match rw.persist_query(text, &canonical_filter, top_fact_id, created_at) {
        Ok(link) => link,
        Err(_) => return Err(operational_err()),
    };
    hits[0].link = Some(link);
    let body = render_envelope(text, filter, &hits);
    Ok(DispatchOutput {
        format: crate::format::OutputFormat::Json,
        body,
    })
}

/// Temporary query execution entry point (tasks 4.3 + 5.2 ordering).
///
/// Opens the index through the read-only snapshot constructor and verifies
/// schema plus all six `model_meta` values before any later model/embedding
/// work. Every storage failure (missing, unreadable, incompatible,
/// unstable snapshot) maps to the single generic operational error above;
/// underlying details are discarded, never propagated. Retrieval, embedding,
/// and success output land in tasks 5.x-6.x, so the verified path still
/// returns the same generic operational error. Never returns
/// `InvalidInput`: argv/filter validation stays in the parser, so
/// status-2 handling is preserved by construction. The ingest read-write
/// opener is never touched here.
pub(crate) fn run(
    root: &Path,
    text: &str,
    filter: &BTreeMap<String, String>,
) -> Result<DispatchOutput, LibraryError> {
    run_with_model_embed(
        root,
        text,
        filter,
        crate::model_cache::cached_model_files,
        |paths, query_text| {
            let embedder = crate::embed::BgeEmbedder::load(paths).map_err(|_| operational_err())?;
            use crate::ingest::Embedder;
            embedder.embed(query_text).map_err(|_| operational_err())
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::Embedder;
    use crate::ingest::HashStore;

    #[test]
    fn stub_is_operational_not_usage() {
        let dir = std::env::temp_dir().join(format!(
            "infraware-q43-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let err = run(&dir, "hello", &BTreeMap::new()).unwrap_err();
        assert!(
            !err.is_usage(),
            "query storage gate must map to status 1, not usage status 2"
        );
        assert_eq!(
            err,
            LibraryError::Io {
                message: QUERY_OPERATIONAL_MESSAGE.to_string(),
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn temp_root_43() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "infraware-q43-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn assert_safe_operational(err: &LibraryError, root: &std::path::Path) {
        assert!(!err.is_usage(), "operational failure must map to status 1");
        assert_eq!(
            *err,
            LibraryError::Io {
                message: QUERY_OPERATIONAL_MESSAGE.to_string(),
            }
        );
        // Rendered exactly as `run` does: one `error:` line, empty stdout.
        let rendered = format!("error: {err}");
        assert_eq!(rendered.lines().count(), 1);
        assert!(rendered.starts_with("error:"));
        for leaked in [
            root.to_string_lossy().into_owned(),
            ".infraware".to_string(),
            ".sqlite".to_string(),
            "vecs".to_string(),
            "SELECT".to_string(),
            "select".to_string(),
            "model".to_string(),
            "revision".to_string(),
            "dimension".to_string(),
            "cosine".to_string(),
        ] {
            assert!(
                !rendered.contains(&leaked),
                "stderr must not leak internal detail {leaked:?}: {rendered:?}"
            );
        }
    }

    #[test]
    fn missing_index_maps_to_safe_operational() {
        let root = temp_root_43();
        let err = run(&root, "hello", &BTreeMap::new()).unwrap_err();
        assert_safe_operational(&err, &root);
        // Missing index creates nothing.
        assert!(!root.join(".infraware").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn incompatible_index_maps_to_safe_operational() {
        let root = temp_root_43();
        {
            let mut w = crate::vec_store::SqliteStore::open(&root).unwrap();
            let emb = vec![0.5f32; crate::ingest::INGEST_DIMENSION as usize];
            w.insert(
                "q1",
                "hello",
                &crate::metadata::canonicalize_metadata(Some(r#"{"a":"1"}"#)).unwrap(),
                &emb,
            )
            .unwrap();
            drop(w);
            let tamper =
                rusqlite::Connection::open(root.join(".infraware").join("vec-index.sqlite"))
                    .unwrap();
            tamper
                .execute(
                    "UPDATE model_meta SET value='768' WHERE key='dimension'",
                    [],
                )
                .unwrap();
        }
        let err = run(&root, "hello", &BTreeMap::new()).unwrap_err();
        assert_safe_operational(&err, &root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unreadable_index_maps_to_safe_operational() {
        let root = temp_root_43();
        // A directory at the DB path is not a readable index file.
        let dir = root.join(".infraware");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join("vec-index.sqlite")).unwrap();
        let err = run(&root, "hello", &BTreeMap::new()).unwrap_err();
        assert_safe_operational(&err, &root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dispatch_query_failures_map_to_safe_operational() {
        let root = temp_root_43();
        let err = crate::dispatch::dispatch(
            crate::Command::Query {
                text: "hello".to_string(),
                filter: BTreeMap::new(),
            },
            &root,
        )
        .unwrap_err();
        assert_safe_operational(&err, &root);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Task 2.5: dispatched query on a missing index fails operationally
    /// and creates no files or directories (no index, no sidecars).
    #[test]
    fn dispatch_query_on_missing_index_creates_nothing() {
        let root = temp_root_43();
        let err = crate::dispatch::dispatch(
            crate::Command::Query {
                text: "hello".to_string(),
                filter: BTreeMap::new(),
            },
            &root,
        )
        .unwrap_err();
        assert_safe_operational(&err, &root);
        let entries: Vec<_> = std::fs::read_dir(&root).unwrap().collect();
        assert!(
            entries.is_empty(),
            "query on missing index must create no files: {entries:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    fn filter_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn empty_hits_envelope_is_exact_compact_string() {
        let out = render_envelope("hello", &BTreeMap::new(), &[]);
        assert_eq!(
            out,
            r#"{"query":"hello","filter":{},"max_distance":0.15,"hits":[]}"#
        );
    }

    #[test]
    fn envelope_field_order_and_max_distance_value() {
        let out = render_envelope("hello", &BTreeMap::new(), &[]);
        // Exact key order at the top level; numeric threshold verbatim.
        let qi = out.find("\"query\"").unwrap();
        let fi = out.find("\"filter\"").unwrap();
        let mi = out.find("\"max_distance\"").unwrap();
        let hi = out.find("\"hits\"").unwrap();
        assert!(
            qi < fi && fi < mi && mi < hi,
            "field order must hold: {out}"
        );
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["max_distance"], serde_json::json!(0.15));
        assert_eq!(v.as_object().unwrap().len(), 4);
        // No pretty whitespace: single line, no trailing newline.
        assert!(!out.contains('\n'));
        assert!(!out.contains(": "));
    }

    #[test]
    fn canonical_filter_output_byte_order() {
        let filter = filter_of(&[("b", "2"), ("a", "1")]);
        let out = render_envelope("hello", &filter, &[]);
        assert_eq!(
            out,
            r#"{"query":"hello","filter":{"a":"1","b":"2"},"max_distance":0.15,"hits":[]}"#
        );
    }

    #[test]
    fn preserves_query_text_bytes() {
        let out = render_envelope("  hello world  ", &BTreeMap::new(), &[]);
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["query"], serde_json::json!("  hello world  "));
        assert!(out.contains(r#""query":"  hello world  ""#));
    }

    #[test]
    fn representative_hit_field_order_and_shape() {
        let hits = vec![QueryHit {
            id: 3,
            score: 0.9,
            distance: 0.1,
            metadata: filter_of(&[("b", "2"), ("a", "1")]),
            text: "row text".to_string(),
            link: None,
        }];
        let out = render_envelope("hello", &BTreeMap::new(), &hits);
        assert_eq!(
            out,
            r#"{"query":"hello","filter":{},"max_distance":0.15,"hits":[{"id":3,"score":0.9,"distance":0.1,"metadata":{"a":"1","b":"2"},"text":"row text"}]}"#
        );
        // Hit key order: id < score < distance < metadata < text.
        let hi = out.find("\"hits\"").unwrap();
        let tail = &out[hi..];
        let idi = tail.find("\"id\"").unwrap();
        let sci = tail.find("\"score\"").unwrap();
        let di = tail.find("\"distance\"").unwrap();
        let mti = tail.find("\"metadata\"").unwrap();
        let ti = tail.find("\"text\"").unwrap();
        assert!(
            idi < sci && sci < di && di < mti && mti < ti,
            "hit field order must hold: {out}"
        );
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let hit = &v["hits"][0];
        assert_eq!(hit.as_object().unwrap().len(), 5);
    }

    #[test]
    fn empty_metadata_object_and_no_extra_fields() {
        let hits = vec![QueryHit {
            id: 1,
            score: 1.0,
            distance: 0.0,
            metadata: BTreeMap::new(),
            text: "t".to_string(),
            link: None,
        }];
        let out = render_envelope("q", &BTreeMap::new(), &hits);
        assert!(out.contains(r#""metadata":{}"#));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v.as_object().unwrap().len(), 4);
        assert_eq!(v["hits"][0].as_object().unwrap().len(), 5);
    }

    fn candidate(id: i64, distance: f64) -> QueryCandidate {
        QueryCandidate {
            id,
            distance,
            metadata: BTreeMap::new(),
            text: format!("text-{id}"),
        }
    }

    #[test]
    fn rank_keeps_exact_boundary_distance() {
        let hits = rank_candidates(&[candidate(1, 0.15)]);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, 1);
        assert_eq!(hits[0].distance, 0.15);
        assert_eq!(hits[0].score, 1.0 - 0.15);
    }

    #[test]
    fn rank_rejects_just_over_threshold() {
        let just_over = f64::from_bits(0.15f64.to_bits() + 1);
        assert!(just_over > QUERY_MAX_DISTANCE);
        let hits = rank_candidates(&[candidate(1, just_over), candidate(2, 0.16)]);
        assert!(hits.is_empty(), "only distances above 0.15: {hits:?}");
    }

    #[test]
    fn rank_score_is_exactly_one_minus_distance() {
        let hits = rank_candidates(&[candidate(7, 0.1), candidate(8, 0.0)]);
        assert_eq!(hits.len(), 2);
        for hit in &hits {
            assert_eq!(hit.score, 1.0 - hit.distance);
        }
    }

    #[test]
    fn rank_orders_by_distance_then_id() {
        let input = vec![
            candidate(9, 0.05),
            candidate(3, 0.05),
            candidate(5, 0.01),
            candidate(4, 0.2),
        ];
        let hits = rank_candidates(&input);
        let ids: Vec<i64> = hits.iter().map(|h| h.id).collect();
        assert_eq!(ids, vec![5, 3, 9]);
    }

    #[test]
    fn rank_empty_input_yields_empty_hits() {
        assert!(rank_candidates(&[]).is_empty());
    }

    // ---- Task 5.2: setup order open/verify -> cached model -> embed ----

    use std::cell::RefCell;
    use std::rc::Rc;

    fn seeded_root_52() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "infraware-q52-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        {
            let mut w = crate::vec_store::SqliteStore::open(&dir).unwrap();
            let emb = vec![0.5f32; crate::ingest::INGEST_DIMENSION as usize];
            w.insert("s52", "hello", r#"{"a":"1"}"#, &emb).unwrap();
        }
        dir
    }

    fn dummy_paths() -> crate::model_cache::ModelPaths {
        crate::model_cache::ModelPaths {
            config: std::path::PathBuf::from("dummy-config"),
            tokenizer: std::path::PathBuf::from("dummy-tokenizer"),
            weights: std::path::PathBuf::from("dummy-weights"),
        }
    }

    #[test]
    fn setup_order_model_before_embed_with_exact_text() {
        let root = seeded_root_52();
        let order: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
        let seen_text: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
        let o2 = Rc::clone(&order);
        let o3 = Rc::clone(&order);
        let t2 = Rc::clone(&seen_text);
        // Task 6.3: zero-vector embed rescores to distance 1.0 (above the
        // inclusive 0.15 bound), so the ranked set is empty and the path
        // succeeds with `hits: []` instead of the pre-6.3 stub error.
        let out = run_with_model_embed(
            &root,
            "  hello world  ",
            &BTreeMap::new(),
            move || {
                o2.borrow_mut().push("model");
                Ok(dummy_paths())
            },
            move |_paths, text| {
                o3.borrow_mut().push("embed");
                *t2.borrow_mut() = text.to_string();
                Ok(vec![0.0f32; 384])
            },
        )
        .expect("empty qualifying set is success");
        assert_eq!(*seen_text.borrow(), "  hello world  ");
        assert_eq!(*order.borrow(), vec!["model", "embed"]);
        assert_eq!(
            out.body,
            r#"{"query":"  hello world  ","filter":{},"max_distance":0.15,"hits":[]}"#
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn incompatible_index_short_circuits_before_model_and_embed() {
        let root = seeded_root_52();
        {
            let tamper =
                rusqlite::Connection::open(root.join(".infraware").join("vec-index.sqlite"))
                    .unwrap();
            tamper
                .execute(
                    "UPDATE model_meta SET value='768' WHERE key='dimension'",
                    [],
                )
                .unwrap();
        }
        let order: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
        let o2 = Rc::clone(&order);
        let o3 = Rc::clone(&order);
        let err = run_with_model_embed(
            &root,
            "hello",
            &BTreeMap::new(),
            move || {
                o2.borrow_mut().push("model");
                Ok(dummy_paths())
            },
            move |_paths, _text| {
                o3.borrow_mut().push("embed");
                Ok(vec![0.0f32; 384])
            },
        )
        .unwrap_err();
        assert!(order.borrow().is_empty());
        assert_safe_operational(&err, &root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_index_short_circuits_before_model_and_embed() {
        let root = temp_root_43();
        let order: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
        let o2 = Rc::clone(&order);
        let o3 = Rc::clone(&order);
        let err = run_with_model_embed(
            &root,
            "hello",
            &BTreeMap::new(),
            move || {
                o2.borrow_mut().push("model");
                Ok(dummy_paths())
            },
            move |_paths, _text| {
                o3.borrow_mut().push("embed");
                Ok(vec![0.0f32; 384])
            },
        )
        .unwrap_err();
        assert!(order.borrow().is_empty());
        assert_safe_operational(&err, &root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn model_failure_short_circuits_before_embed() {
        let root = seeded_root_52();
        let embeds: Rc<RefCell<u32>> = Rc::new(RefCell::new(0));
        let e2 = Rc::clone(&embeds);
        let err = run_with_model_embed(
            &root,
            "hello",
            &BTreeMap::new(),
            move || {
                Err::<crate::model_cache::ModelPaths, _>(LibraryError::MalformedDocument {
                    message: "missing".to_string(),
                })
            },
            move |_paths, _text| {
                *e2.borrow_mut() += 1;
                Ok(vec![0.0f32; 384])
            },
        )
        .unwrap_err();
        assert_eq!(*embeds.borrow(), 0);
        assert_safe_operational(&err, &root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn embed_failure_maps_to_safe_operational() {
        let root = seeded_root_52();
        let err = run_with_model_embed(
            &root,
            "hello",
            &BTreeMap::new(),
            || Ok(dummy_paths()),
            |_paths, _text| {
                Err::<Vec<f32>, _>(LibraryError::Io {
                    message: "tokenize: boom with /secret/path and vecs SELECT".to_string(),
                })
            },
        )
        .unwrap_err();
        assert_safe_operational(&err, &root);
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- Task 5.3: cache/embed/SQLite/vec failures map operationally ----

    #[test]
    fn cached_validation_failure_with_details_maps_to_safe_operational() {
        // Cached-model validation failures (missing/incomplete/corrupt) must
        // surface as the single generic operational error, even when the
        // underlying detail carries paths, SQL, vectors, or network text.
        for inner in [
            LibraryError::MalformedDocument {
                message: "model cache corrupt: /cache/models SELECT vecs revision 384".to_string(),
            },
            LibraryError::Io {
                message: "cache read /tmp/x failed: network download timeout".to_string(),
            },
        ] {
            let root = seeded_root_52();
            let embeds: Rc<RefCell<u32>> = Rc::new(RefCell::new(0));
            let e2 = Rc::clone(&embeds);
            let err = run_with_model_embed(
                &root,
                "hello",
                &BTreeMap::new(),
                move || Err::<crate::model_cache::ModelPaths, _>(inner.clone()),
                move |_paths, _text| {
                    *e2.borrow_mut() += 1;
                    Ok(vec![0.0f32; 384])
                },
            )
            .unwrap_err();
            assert_eq!(
                *embeds.borrow(),
                0,
                "embed must not run after cache failure"
            );
            assert_safe_operational(&err, &root);
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn sqlite_vec_embed_failures_with_details_map_to_safe_operational() {
        // SQLite/vec/embedding failures (including model-load failures, which
        // share the embed stage on the production path) must be stripped to
        // the generic operational error with no leaked internals.
        for detail in [
            "load /cache/model.safetensors failed: SELECT embedding FROM vecs",
            "bert forward: cosine distance vec [0.1, 0.2] network timeout",
            "vec index unreadable at /tmp/snap/vec-index.sqlite revision=abc dimension=384",
        ] {
            let root = seeded_root_52();
            let err = run_with_model_embed(
                &root,
                "hello",
                &BTreeMap::new(),
                || Ok(dummy_paths()),
                |_paths, _text| {
                    Err::<Vec<f32>, _>(LibraryError::Io {
                        message: detail.to_string(),
                    })
                },
            )
            .unwrap_err();
            assert_safe_operational(&err, &root);
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn production_run_with_valid_index_but_missing_cache_is_safe_operational() {
        // Production `run` uses the cached-only accessor. When the model
        // cache is missing/corrupt the result is the generic operational
        // failure (status 1 shape), never usage and never leaked. When the
        // pinned cache is present in the environment, the same call may
        // instead succeed with the exact envelope; both outcomes preserve
        // the stream/status ownership (no usage error, compact JSON body).
        let root = seeded_root_52();
        match run(&root, "hello", &BTreeMap::new()) {
            Err(err) => assert_safe_operational(&err, &root),
            Ok(out) => {
                assert_eq!(out.format, crate::format::OutputFormat::Json);
                let v: serde_json::Value = serde_json::from_str(&out.body).unwrap();
                assert_eq!(v["query"], serde_json::json!("hello"));
                assert_eq!(v["max_distance"], serde_json::json!(0.15));
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- Task 5.4: named fake/counting query embedder ----

    /// Named fake/counting query embedder (task 5.4).
    ///
    /// Reuses the existing [`crate::ingest::Embedder`] trait (no duplicated
    /// production embedding logic, no network/downloads). Counts calls and
    /// captures the query text byte-exactly; interior mutability lets the
    /// `run_with_model_embed` `FnOnce` embed closure delegate to it.
    struct FakeQueryEmbedder {
        calls: Rc<RefCell<u32>>,
        seen_text: Rc<RefCell<String>>,
    }

    impl FakeQueryEmbedder {
        fn new() -> Self {
            Self {
                calls: Rc::new(RefCell::new(0)),
                seen_text: Rc::new(RefCell::new(String::new())),
            }
        }

        fn call_count(&self) -> u32 {
            *self.calls.borrow()
        }

        fn seen_text(&self) -> String {
            self.seen_text.borrow().clone()
        }

        fn embed_via(
            calls: &Rc<RefCell<u32>>,
            seen_text: &Rc<RefCell<String>>,
            text: &str,
        ) -> Result<Vec<f32>, LibraryError> {
            *calls.borrow_mut() += 1;
            *seen_text.borrow_mut() = text.to_string();
            Ok(vec![0.0f32; 384])
        }
    }

    impl crate::ingest::Embedder for FakeQueryEmbedder {
        fn embed(&self, text: &str) -> Result<Vec<f32>, LibraryError> {
            *self.calls.borrow_mut() += 1;
            *self.seen_text.borrow_mut() = text.to_string();
            Ok(vec![0.0f32; 384])
        }
    }

    #[test]
    fn fake_query_embedder_called_once_with_exact_bytes_after_validation() {
        // Happy setup path: valid index + cached-model resolution, so the
        // query embedder runs exactly once and receives the query text
        // byte-exactly (no trim/normalize), after metadata/cached-model
        // validation. No fetcher/network is involved: `resolve` is a plain
        // cached-only closure and the fake performs no I/O.
        let root = seeded_root_52();
        let fake = FakeQueryEmbedder::new();
        let calls = Rc::clone(&fake.calls);
        let seen = Rc::clone(&fake.seen_text);
        let models: Rc<RefCell<u32>> = Rc::new(RefCell::new(0));
        let m2 = Rc::clone(&models);
        let c2 = Rc::clone(&calls);
        let s2 = Rc::clone(&seen);
        let out = run_with_model_embed(
            &root,
            "  hello world  ",
            &BTreeMap::new(),
            move || {
                *m2.borrow_mut() += 1;
                Ok(dummy_paths())
            },
            move |_paths, text| FakeQueryEmbedder::embed_via(&c2, &s2, text),
        )
        .expect("empty qualifying set is success");
        assert_eq!(*models.borrow(), 1, "cached model validated exactly once");
        assert_eq!(*calls.borrow(), 1, "query embedder called exactly once");
        assert_eq!(*seen.borrow(), "  hello world  ");
        assert_eq!(
            out.body,
            r#"{"query":"  hello world  ","filter":{},"max_distance":0.15,"hits":[]}"#
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fake_query_embedder_never_called_on_earlier_failure() {
        // Every pre-embed failure (missing/incompatible index, cached-model
        // failure) must leave the fake embedder at zero calls.
        let missing = temp_root_43();
        let tampered = seeded_root_52();
        {
            let tamper =
                rusqlite::Connection::open(tampered.join(".infraware").join("vec-index.sqlite"))
                    .unwrap();
            tamper
                .execute(
                    "UPDATE model_meta SET value='768' WHERE key='dimension'",
                    [],
                )
                .unwrap();
        }
        let valid = seeded_root_52();
        let cases: Vec<(std::path::PathBuf, bool)> =
            vec![(missing, false), (tampered, false), (valid, true)];
        for (root, model_fails) in cases {
            let fake = FakeQueryEmbedder::new();
            let err = if model_fails {
                run_with_model_embed(
                    &root,
                    "hello",
                    &BTreeMap::new(),
                    || {
                        Err::<crate::model_cache::ModelPaths, _>(LibraryError::MalformedDocument {
                            message: "missing".to_string(),
                        })
                    },
                    |_paths, text| fake.embed(text),
                )
                .unwrap_err()
            } else {
                run_with_model_embed(
                    &root,
                    "hello",
                    &BTreeMap::new(),
                    || Ok(dummy_paths()),
                    |_paths, text| fake.embed(text),
                )
                .unwrap_err()
            };
            assert_eq!(
                fake.call_count(),
                0,
                "embedder must never run on earlier failure: {root:?}"
            );
            assert_eq!(fake.seen_text(), "");
            assert_safe_operational(&err, &root);
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn query_setup_uses_cached_only_path_with_no_network_or_cap() {
        // Structural guard for task 5.3: the query setup path resolves the
        // model via the cached-only accessor only, with no downloader,
        // network, or retrieval cap symbols.
        let src = include_str!("query.rs");
        let src = src.split("#[cfg(test)]").next().unwrap_or(src);
        assert!(
            src.contains("cached_model_files"),
            "query setup must use the cached-only accessor"
        );
        for banned in [
            "cache::ensure_model_files",
            "ensure_with(",
            "ureq",
            "fetch(",
            "download_stage",
            "LIMIT",
            "limit(",
            "TOP ",
            ".take(",
            "max_results",
            "top_k",
            "topk",
        ] {
            assert!(
                !src.contains(banned),
                "query setup must not contain {banned:?}"
            );
        }
        // Fullscan SQL is the only statement text: no bound, no engine
        // distance call, no row-count probe. Check case-sensitively without
        // naming the banned bound keyword itself.
        assert!(
            src.contains("SELECT rowid, embedding FROM vecs"),
            "production read must be the proven fullscan statement"
        );
        assert!(!src.contains("vecs WHERE rowid"), "no rowid probe");
        assert!(
            !src.contains("ORDER BY distance"),
            "no engine distance ordering in production read"
        );
    }

    // ---- Task 6.1: deep reader seam, cosine rescore, fullscan adapter ----
    // ---- Task 6.2: exact metadata AND-filter before rescore ----

    struct FakeReader {
        rows: Vec<(i64, Vec<f32>, BTreeMap<String, String>)>,
        fail: bool,
    }

    impl FakeReader {
        fn ok(rows: Vec<(i64, Vec<f32>)>) -> Self {
            Self {
                rows: rows
                    .into_iter()
                    .map(|(id, emb)| (id, emb, BTreeMap::new()))
                    .collect(),
                fail: false,
            }
        }

        fn ok_meta(rows: Vec<(i64, Vec<f32>, BTreeMap<String, String>)>) -> Self {
            Self { rows, fail: false }
        }
    }

    impl super::EmbeddingReader for FakeReader {
        fn read_embeddings(
            &self,
            filter: &BTreeMap<String, String>,
        ) -> Result<Vec<(i64, Vec<f32>)>, LibraryError> {
            if self.fail {
                return Err(LibraryError::Io {
                    message: "fake read boom".to_string(),
                });
            }
            // Same exact AND semantics as the production adapter: empty
            // filter keeps every row; otherwise every pair must match.
            Ok(self
                .rows
                .iter()
                .filter(|(_, _, meta)| filter.iter().all(|(k, v)| meta.get(k) == Some(v)))
                .map(|(id, emb, _)| (*id, emb.clone()))
                .collect())
        }
    }

    fn meta_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn unit_basis(dim: usize, index: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; dim];
        v[index] = 1.0;
        v
    }

    #[test]
    fn cosine_distance_identity_orthogonal_opposite() {
        let a = unit_basis(4, 0);
        assert!((super::cosine_distance(&a, &a)).abs() < 1e-12);
        let b = unit_basis(4, 1);
        assert!((super::cosine_distance(&a, &b) - 1.0).abs() < 1e-12);
        let neg: Vec<f32> = a.iter().map(|v| -v).collect();
        assert!((super::cosine_distance(&a, &neg) - 2.0).abs() < 1e-12);
    }

    #[test]
    fn rescore_sorts_by_distance_then_id() {
        let q = unit_basis(4, 0);
        let rows = vec![
            (9, unit_basis(4, 0)),
            (3, unit_basis(4, 0)),
            (5, unit_basis(4, 1)),
        ];
        let scored = super::rescore(&q, &rows);
        let ids: Vec<i64> = scored.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![3, 9, 5]);
        assert!(scored[0].distance.abs() < 1e-12);
        assert!((scored[2].distance - 1.0).abs() < 1e-12);
    }

    // ---- Task 2.3: same-type gate, no cross-type distance ----

    #[test]
    fn typed_gate_rejects_query_vs_query_as_operational() {
        // A query-vs-query comparison is rejected (never computed). The
        // rejection is operational, never usage: it must map to status 1.
        let a = unit_basis(4, 0);
        let b = unit_basis(4, 1);
        let err = super::typed_cosine_distance(
            &super::TypedEmbedding::query(&a),
            &super::TypedEmbedding::query(&b),
        )
        .unwrap_err();
        assert!(!err.is_usage(), "cross-type rejection must be operational");
        // No paths, SQL, or vector contents in the diagnostic token.
        let rendered = format!("error: {err}");
        assert_eq!(rendered.lines().count(), 1);
        assert!(!rendered.contains(".sqlite"));
        assert!(!rendered.contains("SELECT"));
    }

    #[test]
    fn typed_gate_allows_query_vs_fact_and_fact_vs_fact() {
        // The only valid pairings compute the exact cosine distance.
        let q = unit_basis(4, 0);
        let f_same = unit_basis(4, 0);
        let f_other = unit_basis(4, 1);
        let d = super::typed_cosine_distance(
            &super::TypedEmbedding::query(&q),
            &super::TypedEmbedding::fact(&f_same),
        )
        .unwrap();
        assert!(d.abs() < 1e-12);
        // Either operand order is accepted for query-vs-fact.
        let d = super::typed_cosine_distance(
            &super::TypedEmbedding::fact(&f_other),
            &super::TypedEmbedding::query(&q),
        )
        .unwrap();
        assert!((d - 1.0).abs() < 1e-12);
        // Fact-vs-fact (dedup pairing) is accepted with identical math.
        let d = super::typed_cosine_distance(
            &super::TypedEmbedding::fact(&f_same),
            &super::TypedEmbedding::fact(&f_other),
        )
        .unwrap();
        assert!((d - super::cosine_distance(&f_same, &f_other)).abs() < 1e-12);
    }

    #[test]
    fn rescore_never_ranks_a_query_embedding_as_a_candidate() {
        // Integration shape: candidates come only from the reader (stored
        // fact vectors); the live query embedding is the comparator, never
        // a candidate. Every scored id traces back to a fact rowid, and the
        // scored count equals the reader row count (no query-query ranking
        // can add or substitute rows).
        let dim = 4;
        let query_vec = unit_basis(dim, 0);
        let rows = vec![
            (7, unit_basis(dim, 0)),
            (3, unit_basis(dim, 1)),
            (5, unit_basis(dim, 0)),
        ];
        let reader = FakeReader::ok(rows.clone());
        let scored = super::retrieve_scored(&reader, &query_vec, &BTreeMap::new()).unwrap();
        assert_eq!(scored.len(), rows.len());
        let mut got: Vec<i64> = scored.iter().map(|r| r.id).collect();
        got.sort_unstable();
        assert_eq!(got, vec![3, 5, 7]);
        // Same distances as the direct typed gate: ranking used the fact
        // rows against the query, not queries against each other.
        for row in &scored {
            let fact_vec = rows.iter().find(|(id, _)| *id == row.id).unwrap().1.clone();
            let expected = super::typed_cosine_distance(
                &super::TypedEmbedding::query(&query_vec),
                &super::TypedEmbedding::fact(&fact_vec),
            )
            .unwrap();
            assert!((row.distance - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn fake_reader_iterates_all_rows_beyond_probe_k() {
        // Six rows: strictly above the k=5 probe, proving no silent
        // truncation at the seam level.
        let dim = 384;
        let unit: Vec<f32> = {
            let v = 1.0f32 / (dim as f32).sqrt();
            vec![v; dim]
        };
        let rows: Vec<(i64, Vec<f32>)> = (1..=6).map(|id| (id, unit.clone())).collect();
        let reader = FakeReader::ok(rows);
        let scored = super::retrieve_scored(&reader, &unit, &BTreeMap::new()).unwrap();
        assert_eq!(scored.len(), 6);
        let ids: Vec<i64> = scored.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5, 6]);
        for r in &scored {
            assert!(r.distance.abs() < 1e-6, "distance was {}", r.distance);
        }
    }

    #[test]
    fn fake_reader_failure_propagates_for_operational_mapping() {
        let reader = FakeReader {
            rows: vec![],
            fail: true,
        };
        assert!(super::retrieve_scored(&reader, &[1.0f32, 0.0], &BTreeMap::new()).is_err());
    }

    #[test]
    fn production_adapter_fullscan_returns_all_six_rows() {
        // Real `SqliteStore` adapter over N=6 rows (above probe k=5):
        // identical deterministic unit embeddings, fullscan count == 6.
        let root = temp_root_43();
        let dim = crate::ingest::INGEST_DIMENSION as usize;
        let unit: Vec<f32> = {
            let v = 1.0f32 / (dim as f32).sqrt();
            vec![v; dim]
        };
        {
            let mut w = crate::vec_store::SqliteStore::open(&root).unwrap();
            for i in 0..6 {
                w.insert(
                    &format!("q61-{i}"),
                    &format!("doc {i:02}"),
                    r#"{"group":"fixture16"}"#,
                    &unit,
                )
                .unwrap();
            }
        }
        let ro = crate::vec_store::SqliteStore::open_read_only(&root).unwrap();
        let rows = super::EmbeddingReader::read_embeddings(&ro, &BTreeMap::new()).unwrap();
        assert_eq!(rows.len(), 6, "fullscan must iterate all N rows");
        for (_, emb) in &rows {
            assert_eq!(emb.len(), dim);
        }
        let scored = super::retrieve_scored(&ro, &unit, &BTreeMap::new()).unwrap();
        assert_eq!(scored.len(), 6);
        let ids: Vec<i64> = scored.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5, 6]);
        drop(ro);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fake_filter_excludes_partial_match_before_rank() {
        // Two near-identical rows (both distance 0.0); only the fully
        // matching row may reach rescore/ranking.
        let dim = 4;
        let unit = unit_basis(dim, 0);
        let reader = FakeReader::ok_meta(vec![
            (1, unit.clone(), meta_of(&[("a", "1"), ("b", "2")])),
            (2, unit.clone(), meta_of(&[("a", "1"), ("b", "WRONG")])),
        ]);
        let filter = meta_of(&[("a", "1"), ("b", "2")]);
        let scored = super::retrieve_scored(&reader, &unit, &filter).unwrap();
        assert_eq!(scored.len(), 1);
        assert_eq!(scored[0].id, 1);
    }

    #[test]
    fn fake_empty_filter_keeps_all_and_multi_key_requires_all_pairs() {
        let dim = 4;
        let unit = unit_basis(dim, 0);
        let reader = FakeReader::ok_meta(vec![
            (1, unit.clone(), meta_of(&[("a", "1"), ("b", "2")])),
            (2, unit.clone(), meta_of(&[("a", "1")])),
            (3, unit.clone(), meta_of(&[("b", "2")])),
        ]);
        let all = super::retrieve_scored(&reader, &unit, &BTreeMap::new()).unwrap();
        assert_eq!(all.len(), 3);
        let both =
            super::retrieve_scored(&reader, &unit, &meta_of(&[("a", "1"), ("b", "2")])).unwrap();
        assert_eq!(both.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1]);
        let single = super::retrieve_scored(&reader, &unit, &meta_of(&[("a", "1")])).unwrap();
        assert_eq!(single.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1, 2]);
        let none = super::retrieve_scored(&reader, &unit, &meta_of(&[("a", "nope")])).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn production_adapter_filters_before_rescore_with_near_distractor() {
        // Real SQLite: identical unit embeddings (all distance 0.0) so any
        // exclusion must come from the `meta_kv` filter, not ranking. Row 2
        // is a near distractor: matches one of two keys only.
        let root = temp_root_43();
        let dim = crate::ingest::INGEST_DIMENSION as usize;
        let unit: Vec<f32> = {
            let v = 1.0f32 / (dim as f32).sqrt();
            vec![v; dim]
        };
        {
            let mut w = crate::vec_store::SqliteStore::open(&root).unwrap();
            w.insert("qf-1", "doc one", r#"{"a":"1","b":"2"}"#, &unit)
                .unwrap();
            w.insert("qf-2", "doc two", r#"{"a":"1","b":"WRONG"}"#, &unit)
                .unwrap();
            w.insert("qf-3", "doc three", r#"{"a":"1"}"#, &unit)
                .unwrap();
        }
        let ro = crate::vec_store::SqliteStore::open_read_only(&root).unwrap();
        let all = super::retrieve_scored(&ro, &unit, &BTreeMap::new()).unwrap();
        assert_eq!(all.len(), 3);
        let both = super::retrieve_scored(&ro, &unit, &meta_of(&[("a", "1"), ("b", "2")])).unwrap();
        assert_eq!(both.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1]);
        let single = super::retrieve_scored(&ro, &unit, &meta_of(&[("a", "1")])).unwrap();
        assert_eq!(
            single.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        let narrow = super::retrieve_scored(&ro, &unit, &meta_of(&[("b", "2")])).unwrap();
        assert_eq!(narrow.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1]);
        drop(ro);
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- Task 6.3: threshold/rank/post-rank read/success envelope ----

    fn scored_of(pairs: &[(i64, f64)]) -> Vec<super::ScoredRow> {
        pairs
            .iter()
            .map(|(id, d)| super::ScoredRow {
                id: *id,
                distance: *d,
            })
            .collect()
    }

    fn record_map(text: &str) -> (BTreeMap<String, String>, String) {
        (BTreeMap::new(), text.to_string())
    }

    #[test]
    fn post_rank_far_rows_excluded_and_boundary_kept() {
        let scored = scored_of(&[(1, 0.0), (2, 0.15), (3, 0.5), (4, 1.0)]);
        let seen: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(Vec::new()));
        let s2 = Rc::clone(&seen);
        let hits = super::assemble_hits(&scored, move |id| {
            s2.borrow_mut().push(id);
            Ok(record_map(&format!("text-{id}")))
        })
        .unwrap();
        assert_eq!(hits.iter().map(|h| h.id).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(*seen.borrow(), vec![1, 2]);
        assert_eq!(hits[1].distance, 0.15);
        assert_eq!(hits[1].score, 1.0 - 0.15);
    }

    #[test]
    fn post_rank_read_receives_only_qualifying_ordered_ids() {
        // Input rescore order is distance-then-id; far id 7 must never be read.
        let scored = scored_of(&[(9, 0.05), (3, 0.05), (5, 0.01), (7, 0.9)]);
        let seen: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(Vec::new()));
        let s2 = Rc::clone(&seen);
        let hits = super::assemble_hits(&scored, move |id| {
            s2.borrow_mut().push(id);
            Ok(record_map(&format!("text-{id}")))
        })
        .unwrap();
        assert_eq!(*seen.borrow(), vec![5, 3, 9]);
        assert_eq!(hits.iter().map(|h| h.id).collect::<Vec<_>>(), vec![5, 3, 9]);
        for h in &hits {
            assert_eq!(h.score, 1.0 - h.distance);
        }
    }

    #[test]
    fn post_rank_literal_just_over_threshold_excluded() {
        // Task 3.1 traceability: the literal task-text value 0.15001 is
        // excluded by the inclusive <= 0.15 bound (existing next-ULP test
        // proves the tightest float boundary; this pins the spec literal).
        let scored = scored_of(&[(1, 0.15), (2, 0.15001)]);
        let ids = super::ranked_qualifying_ids(&scored);
        assert_eq!(ids.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn differing_metadata_identical_vectors_rank_equally_lowest_id_first() {
        // Task 3.1: metadata is ignored during cosine comparison — only
        // the stored vector participates. Identical vectors with different
        // metadata must score equally and order by lowest integer ID.
        let dim = 4;
        let unit = unit_basis(dim, 0);
        let reader = FakeReader::ok_meta(vec![
            (2, unit.clone(), meta_of(&[("a", "1")])),
            (1, unit.clone(), meta_of(&[("a", "2"), ("b", "X")])),
        ]);
        let scored = super::retrieve_scored(&reader, &unit, &BTreeMap::new()).unwrap();
        assert_eq!(scored.len(), 2);
        for row in &scored {
            assert!(row.distance.abs() < 1e-12, "distance was {}", row.distance);
        }
        assert!((scored[0].distance - scored[1].distance).abs() < 1e-12);
        // Equal distances fall back to lowest-ID order despite reverse input.
        assert_eq!(scored.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1, 2]);
        let seen: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(Vec::new()));
        let s2 = Rc::clone(&seen);
        let hits = super::assemble_hits(&scored, move |id| {
            s2.borrow_mut().push(id);
            let meta = if id == 1 {
                meta_of(&[("a", "2"), ("b", "X")])
            } else {
                meta_of(&[("a", "1")])
            };
            Ok((meta, format!("text-{id}")))
        })
        .unwrap();
        assert_eq!(hits.iter().map(|h| h.id).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(*seen.borrow(), vec![1, 2]);
        for h in &hits {
            assert_eq!(h.score, 1.0 - h.distance);
        }
        // Payload metadata survives per-ID even though it never entered math.
        assert_eq!(hits[0].metadata, meta_of(&[("a", "2"), ("b", "X")]));
        assert_eq!(hits[1].metadata, meta_of(&[("a", "1")]));
    }

    #[test]
    fn near_distractor_filtered_before_rank_never_reaches_post_rank() {
        let dim = 4;
        let unit = unit_basis(dim, 0);
        let reader = FakeReader::ok_meta(vec![
            (1, unit.clone(), meta_of(&[("a", "1"), ("b", "2")])),
            (2, unit.clone(), meta_of(&[("a", "1"), ("b", "WRONG")])),
        ]);
        let filter = meta_of(&[("a", "1"), ("b", "2")]);
        let scored = super::retrieve_scored(&reader, &unit, &filter).unwrap();
        assert_eq!(scored.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1]);
        let seen: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(Vec::new()));
        let s2 = Rc::clone(&seen);
        let out = super::render_success("hello", &filter, &scored, move |id| {
            s2.borrow_mut().push(id);
            Ok((meta_of(&[("a", "1"), ("b", "2")]), "doc one".to_string()))
        })
        .unwrap();
        assert_eq!(*seen.borrow(), vec![1]);
        assert_eq!(
            out.body,
            r#"{"query":"hello","filter":{"a":"1","b":"2"},"max_distance":0.15,"hits":[{"id":1,"score":1.0,"distance":0.0,"metadata":{"a":"1","b":"2"},"text":"doc one"}]}"#
        );
    }

    #[test]
    fn empty_qualifying_set_renders_exact_empty_hits() {
        let scored = scored_of(&[(1, 0.5), (2, 1.0)]);
        let calls: Rc<RefCell<u32>> = Rc::new(RefCell::new(0));
        let c2 = Rc::clone(&calls);
        let out = super::render_success("hello", &BTreeMap::new(), &scored, move |id| {
            *c2.borrow_mut() += 1;
            Ok(record_map(&format!("text-{id}")))
        })
        .unwrap();
        assert_eq!(*calls.borrow(), 0);
        assert_eq!(
            out.body,
            r#"{"query":"hello","filter":{},"max_distance":0.15,"hits":[]}"#
        );
        assert_eq!(out.format, crate::format::OutputFormat::Json);
    }

    #[test]
    fn missing_post_rank_record_propagates_for_operational_mapping() {
        let scored = scored_of(&[(1, 0.0)]);
        let res = super::assemble_hits(&scored, |_| {
            Err(LibraryError::Io {
                message: "gone".to_string(),
            })
        });
        assert!(res.is_err());
    }

    #[test]
    fn production_success_path_with_fake_embedder_and_cache_resolver() {
        // Real `SqliteStore` adapter, no network/model: two unit rows qualify
        // (distance 0.0), one orthogonal row is far (distance 1.0).
        let root = temp_root_43();
        let dim = crate::ingest::INGEST_DIMENSION as usize;
        let unit: Vec<f32> = {
            let v = 1.0f32 / (dim as f32).sqrt();
            vec![v; dim]
        };
        let mut ortho = vec![0.0f32; dim];
        ortho[0] = 1.0;
        {
            let mut w = crate::vec_store::SqliteStore::open(&root).unwrap();
            w.insert("qs-1", "doc one", r#"{"a":"1"}"#, &unit).unwrap();
            w.insert("qs-2", "doc two", r#"{"a":"1"}"#, &unit).unwrap();
            w.insert("qs-3", "far doc", r#"{"a":"1"}"#, &ortho).unwrap();
        }
        let filter = meta_of(&[("a", "1")]);
        let unit2 = unit.clone();
        let out = super::run_with_model_embed(
            &root,
            "hello",
            &filter,
            || Ok(dummy_paths()),
            move |_paths, _text| Ok(unit2.clone()),
        )
        .expect("qualifying rows must succeed");
        let v: serde_json::Value = serde_json::from_str(&out.body).unwrap();
        assert_eq!(v["query"], serde_json::json!("hello"));
        assert_eq!(v["filter"], serde_json::json!({"a": "1"}));
        assert_eq!(v["max_distance"], serde_json::json!(0.15));
        let ids: Vec<i64> = v["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["id"].as_i64().unwrap())
            .collect();
        assert_eq!(ids, vec![1, 2]);
        for h in v["hits"].as_array().unwrap() {
            assert_eq!(h["score"], serde_json::json!(1.0));
            assert_eq!(h["distance"], serde_json::json!(0.0));
        }
        assert_eq!(v["hits"][0]["text"], serde_json::json!("doc one"));
        assert_eq!(v["hits"][1]["text"], serde_json::json!("doc two"));
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- Task 6.4: statements read-only and never held across embedding ----

    #[test]
    fn statements_never_held_across_embedding() {
        // Ordering: schema/meta validation finishes before cached-model
        // resolution/embedding, and retrieval statements are created only
        // after embedding returns. All in `run_with_model_embed`.
        let src = include_str!("query.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap_or(src);
        let body = prod
            .split("pub(crate) fn run_with_model_embed")
            .nth(1)
            .expect("run_with_model_embed must exist");
        // Cut before the test module re-opens the name in comments only;
        // the first `pub(crate) fn run(` ends the setup body.
        let body = body.split("pub(crate) fn run(").next().unwrap_or(body);
        let i_verify = body
            .find("verify_query_compatible")
            .expect("must verify first");
        let i_resolve = body.find("resolve()").expect("must resolve model");
        let i_embed = body.find("embed(").expect("must embed");
        let i_scan = body
            .find("retrieve_scored")
            .expect("must retrieve after embed");
        assert!(
            i_verify < i_resolve && i_resolve < i_embed && i_embed < i_scan,
            "order must be verify -> resolve -> embed -> retrieve, got {i_verify} < {i_resolve} < {i_embed} < {i_scan}"
        );
        // Query layer itself never touches SQLite: no Statement, borrow, or
        // rows iterator can be held across embedding by construction.
        for banned in ["Statement", "prepare", "query_row", "query_map", "rusqlite"] {
            assert!(
                !body.contains(banned),
                "query setup body must not contain {banned:?}"
            );
        }
        // Store layer: no Statement/rows held on the struct; validation is
        // `query_row`-only (no prepare), scans/records scope statements
        // locally and drain them inside the call.
        let vsrc = include_str!("vec_store.rs");
        let vprod = vsrc.split("mod tests").next().unwrap_or(vsrc);
        assert!(
            !vprod.contains("Statement"),
            "store must not store or return a Statement"
        );
        // `QueryReturnedNoRows` is a legitimate error variant (task 5.3),
        // not a held iterator; scrub it before checking for the Rows type.
        let scrubbed = vprod.replace("QueryReturnedNoRows", "");
        assert!(
            !scrubbed.contains("Rows"),
            "store must not leak a rows iterator"
        );
        let verify = vprod
            .split("fn verify_query_compatible")
            .nth(1)
            .expect("verify fn must exist")
            .split("fn scan_embeddings")
            .next()
            .unwrap_or("");
        assert!(
            !verify.contains("prepare"),
            "validation must be query_row-only, no prepared statement to hold"
        );
        assert!(
            verify.contains("query_row"),
            "validation reads via scoped query_row"
        );
        let scan = vprod
            .split("fn scan_filtered_embeddings")
            .nth(1)
            .expect("scan fn must exist")
            .split("fn read_record")
            .next()
            .unwrap_or("");
        assert!(
            scan.contains("let mut stmt"),
            "retrieval statement is a call-local binding"
        );
        assert!(
            scan.contains("for row in rows"),
            "retrieval drains rows inside the same call"
        );
        // Retrieval SQL itself is read-only.
        assert!(scan.contains("SELECT rowid, embedding FROM vecs"));
        for banned in ["INSERT", "UPDATE", "DELETE", "CREATE", "LIMIT"] {
            assert!(
                !scan.contains(banned),
                "retrieval must stay read-only with no {banned}"
            );
        }
    }

    // ---- Task 6.5: durable N=6 fixture beyond the k=5 probe ----
    //
    // Pinned plan (design section 6): N=6 deterministic 384-f32 unit
    // embeddings, every row a qualifying hit at distance ~0, deterministic
    // text `doc {00..05}`, metadata `{"group":"fixture16","seq":"{00..05}"}`.
    // Fullscan iteration count == 6, final hit count == 6, IDs [1..6].
    // Loud-failure rule: fewer rows without an explicit operational error
    // fails loudly, never accepted as a cap. The finite-k LIMIT 5 control
    // below is a truncation regression guard only, never production
    // retrieval (production stays the fullscan + Rust rescore path).

    fn fixture16_unit_384() -> Vec<f32> {
        let dim = crate::ingest::INGEST_DIMENSION as usize;
        debug_assert_eq!(dim, 384);
        let v = 1.0f32 / (dim as f32).sqrt();
        vec![v; dim]
    }

    fn fixture16_text(i: usize) -> String {
        format!("doc {i:02}")
    }

    fn fixture16_meta(i: usize) -> BTreeMap<String, String> {
        meta_of(&[("group", "fixture16"), ("seq", &format!("{i:02}"))])
    }

    fn fixture16_canonical(i: usize) -> String {
        format!(r#"{{"group":"fixture16","seq":"{i:02}"}}"#)
    }

    fn fixture16_fake_rows() -> Vec<(i64, Vec<f32>, BTreeMap<String, String>)> {
        let unit = fixture16_unit_384();
        (0..6)
            .map(|i| (i as i64 + 1, unit.clone(), fixture16_meta(i)))
            .collect()
    }

    /// Seed the real pinned SQLite index for task 6.5 via the ingest opener
    /// (fixture setup only). Returns the root; caller opens read-only.
    fn fixture16_seed_real_index() -> std::path::PathBuf {
        let root = temp_root_43();
        let unit = fixture16_unit_384();
        let mut w = crate::vec_store::SqliteStore::open(&root).unwrap();
        for i in 0..6 {
            w.insert(
                &format!("fixture16-{i:02}"),
                &fixture16_text(i),
                &fixture16_canonical(i),
                &unit,
            )
            .unwrap();
        }
        drop(w);
        root
    }

    #[test]
    fn fixture16_durable_fake_reader_six_hits_no_truncation() {
        // Durable fake-reader proof: N=6 (strictly above probe k=5),
        // all unit embeddings so every row qualifies at distance ~0.
        let unit = fixture16_unit_384();
        let reader = FakeReader::ok_meta(fixture16_fake_rows());
        let scored = super::retrieve_scored(&reader, &unit, &BTreeMap::new()).unwrap();
        assert_eq!(
            scored.len(),
            6,
            "LOUD FAILURE: fullscan iteration returned {} rows, expected 6; \
             fewer rows without an explicit operational error is never a cap",
            scored.len()
        );
        let ids: Vec<i64> = scored.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5, 6]);
        for r in &scored {
            assert!(
                r.distance.abs() < 1e-6,
                "every fixture row must be a qualifying hit at distance ~0, got {}",
                r.distance
            );
        }
        // Success path: threshold keeps all 6, ordered by (distance, id).
        let seen: Rc<RefCell<Vec<i64>>> = Rc::new(RefCell::new(Vec::new()));
        let s2 = Rc::clone(&seen);
        let out = super::render_success("hello", &BTreeMap::new(), &scored, move |id| {
            s2.borrow_mut().push(id);
            let idx = (id - 1) as usize;
            Ok((fixture16_meta(idx), fixture16_text(idx)))
        })
        .unwrap();
        assert_eq!(*seen.borrow(), vec![1, 2, 3, 4, 5, 6]);
        let v: serde_json::Value = serde_json::from_str(&out.body).unwrap();
        assert_eq!(
            v["hits"].as_array().unwrap().len(),
            6,
            "LOUD FAILURE: final hit count is not 6"
        );
        let hit_ids: Vec<i64> = v["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["id"].as_i64().unwrap())
            .collect();
        assert_eq!(hit_ids, vec![1, 2, 3, 4, 5, 6]);
        for (n, h) in v["hits"].as_array().unwrap().iter().enumerate() {
            assert_eq!(h["text"], serde_json::json!(fixture16_text(n)));
            assert_eq!(h["metadata"]["group"], serde_json::json!("fixture16"));
            assert_eq!(h["metadata"]["seq"], serde_json::json!(format!("{n:02}")));
        }
        // Metadata AND-filtering stays before ranking: narrow to seq=03.
        let narrow = super::retrieve_scored(
            &reader,
            &unit,
            &meta_of(&[("group", "fixture16"), ("seq", "03")]),
        )
        .unwrap();
        assert_eq!(
            narrow.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![4],
            "AND filter must leave only seq=03 (id 4) before ranking"
        );
    }

    #[test]
    fn fixture16_pinned_sqlite_fixture_six_hits_no_truncation() {
        // Real pinned stack (Rust/rusqlite/sqlite-vec via SqliteStore): N=6
        // rows with deterministic text/metadata and identical unit
        // embeddings. Exercises reader + ranking + success path with
        // injected cached-model resolver and embedder (no network/model).
        let root = fixture16_seed_real_index();
        let unit = fixture16_unit_384();
        let ro = crate::vec_store::SqliteStore::open_read_only(&root).unwrap();
        // Fullscan iteration count == 6.
        let rows = super::EmbeddingReader::read_embeddings(&ro, &BTreeMap::new()).unwrap();
        assert_eq!(
            rows.len(),
            6,
            "LOUD FAILURE: fullscan iteration returned {} rows, expected 6; \
             fewer rows without an explicit operational error is never a cap",
            rows.len()
        );
        // Rescore: all 6 qualify at distance ~0, IDs in order.
        let scored = super::retrieve_scored(&ro, &unit, &BTreeMap::new()).unwrap();
        assert_eq!(
            scored.len(),
            6,
            "LOUD FAILURE: rescore returned {} rows, expected 6",
            scored.len()
        );
        assert_eq!(
            scored.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6]
        );
        for r in &scored {
            assert!(
                r.distance.abs() < 1e-6,
                "fixture distance must be ~0, got {}",
                r.distance
            );
        }
        drop(ro);
        // Full query path with injected embed/cache: final hit count == 6.
        let unit2 = unit.clone();
        let out = super::run_with_model_embed(
            &root,
            "hello",
            &BTreeMap::new(),
            || Ok(dummy_paths()),
            move |_paths, _text| Ok(unit2.clone()),
        )
        .expect("all 6 fixture rows qualify: success");
        let v: serde_json::Value = serde_json::from_str(&out.body).unwrap();
        let hits = v["hits"].as_array().unwrap();
        assert_eq!(
            hits.len(),
            6,
            "LOUD FAILURE: final hit count is {} not 6",
            hits.len()
        );
        let ids: Vec<i64> = hits.iter().map(|h| h["id"].as_i64().unwrap()).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5, 6]);
        for (n, h) in hits.iter().enumerate() {
            assert_eq!(h["text"], serde_json::json!(fixture16_text(n)));
            assert_eq!(h["metadata"]["group"], serde_json::json!("fixture16"));
            assert_eq!(h["metadata"]["seq"], serde_json::json!(format!("{n:02}")));
        }
        // Metadata AND-filtering remains before ranking on the real index.
        let unit3 = unit.clone();
        let filtered = super::run_with_model_embed(
            &root,
            "hello",
            &meta_of(&[("group", "fixture16"), ("seq", "03")]),
            || Ok(dummy_paths()),
            move |_paths, _text| Ok(unit3.clone()),
        )
        .unwrap();
        let fv: serde_json::Value = serde_json::from_str(&filtered.body).unwrap();
        let fhits = fv["hits"].as_array().unwrap();
        assert_eq!(fhits.len(), 1);
        assert_eq!(fhits[0]["id"], serde_json::json!(4));
        assert_eq!(fhits[0]["text"], serde_json::json!("doc 03"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fixture16_limit5_truncation_control_guard() {
        // Truncation regression guard ONLY: a finite-k LIMIT 5 read over the
        // same N=6 fixture returns 5 < 6, documenting that k=5 truncates.
        // Never used in production retrieval (production is the unbounded
        // `SELECT rowid, embedding FROM vecs` fullscan + Rust rescore).
        let root = fixture16_seed_real_index();
        let ro = crate::vec_store::SqliteStore::open_read_only(&root).unwrap();
        let rows = super::EmbeddingReader::read_embeddings(&ro, &BTreeMap::new()).unwrap();
        assert_eq!(
            rows.len(),
            6,
            "LOUD FAILURE: control fixture must hold 6 rows, got {}",
            rows.len()
        );
        drop(ro);
        // Raw control query with an explicit finite k; confined to this test.
        {
            let ro2 = crate::vec_store::SqliteStore::open_read_only(&root).unwrap();
            // Reach the snapshot connection indirectly is not exposed, so
            // open the stored snapshot path read-only for the control read.
            let snap = ro2.snapshot_path().expect("snapshot must exist");
            let db = snap.join("vec-index.sqlite");
            let c = rusqlite::Connection::open_with_flags(
                &db,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let n: i64 = c
                .query_row("SELECT count(*) FROM vecs", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 6, "LOUD FAILURE: control count must be 6");
            let limited: Vec<i64> = c
                .prepare("SELECT rowid FROM vecs LIMIT 5")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            assert_eq!(
                limited.len(),
                5,
                "LIMIT 5 control must truncate the N=6 fixture to 5"
            );
            assert!(limited.len() < 6);
            drop(c);
            drop(ro2);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- Task 7.3: empty-hit success, canonical output, no leaks, fingerprints ----

    /// Byte-exact fingerprint of the four query-relevant artifacts for 7.3:
    /// main DB plus `-wal`/`-shm`/`-journal` siblings. Mirrors the task 4.4
    /// helper in `vec_store` tests; `None` means absent, `Some(bytes)` is
    /// the full file content.
    fn q73_fingerprint(root: &std::path::Path) -> Vec<(String, Option<Vec<u8>>)> {
        let main = root.join(".infraware").join("vec-index.sqlite");
        let base = main.as_os_str().to_owned();
        let mut out = Vec::new();
        out.push(("vec-index.sqlite".to_string(), std::fs::read(&main).ok()));
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut s = base.clone();
            s.push(suffix);
            let p = std::path::PathBuf::from(s);
            out.push((format!("vec-index.sqlite{suffix}"), std::fs::read(&p).ok()));
        }
        out
    }

    fn q73_assert_no_leaks(rendered: &str, root: &std::path::Path) {
        let root_s = root.to_string_lossy().into_owned();
        for leaked in [
            root_s.as_str(),
            ".infraware",
            ".sqlite",
            "vec-index",
            "vecs",
            "SELECT",
            "select",
            "embedding",
            "model",
            "cache",
            "revision",
            "dimension",
            "metric",
            "cosine",
            "768",
            "384",
        ] {
            assert!(
                !rendered.contains(leaked),
                "output/diagnostic must not leak {leaked:?}: {rendered:?}"
            );
        }
    }

    #[test]
    fn q73_empty_hits_success_exact_compact_and_artifacts_unchanged() {
        // Actual read-only success path with injected fake cache/embed (no
        // network/weights): orthogonal embed rescores to distance 1.0, so no
        // row qualifies. Proves status-0 shape (`Ok` + exact compact
        // `hits:[]`) and byte-identical originals.
        let root = fixture16_seed_real_index();
        let before = q73_fingerprint(&root);
        assert!(before[0].1.is_some(), "main DB must exist before query");
        let dim = crate::ingest::INGEST_DIMENSION as usize;
        let mut ortho = vec![0.0f32; dim];
        ortho[0] = 1.0;
        let out = super::run_with_model_embed(
            &root,
            "hello",
            &BTreeMap::new(),
            || Ok(dummy_paths()),
            move |_paths, _text| Ok(ortho.clone()),
        )
        .expect("empty qualifying set is status-0 success");
        // Exact compact envelope, single line, no pretty whitespace.
        assert_eq!(
            out.body,
            r#"{"query":"hello","filter":{},"max_distance":0.15,"hits":[]}"#
        );
        assert!(out.body.contains("\"hits\":[]"));
        assert!(!out.body.contains('\n'));
        assert!(!out.body.contains(": "));
        assert_eq!(out.format, crate::format::OutputFormat::Json);
        // Success output carries no diagnostics/paths/vectors/model details.
        q73_assert_no_leaks(&out.body, &root);
        // Operational rendering of the error constant stays leak-free too.
        let rendered = format!("error: {}", super::QUERY_OPERATIONAL_MESSAGE);
        assert_eq!(rendered.lines().count(), 1);
        q73_assert_no_leaks(&rendered, &root);
        let after = q73_fingerprint(&root);
        assert_eq!(
            before, after,
            "successful read must leave originals byte-identical"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn persistence_abort_is_redacted_operational_failure() {
        let root = fixture16_seed_real_index();
        let store = crate::vec_store::SqliteStore::open_existing_rw(&root).unwrap();
        store
            .execute_test_sql(
                "CREATE TRIGGER abort_query_persistence BEFORE INSERT ON queries
                 BEGIN SELECT RAISE(ABORT, 'secret query persistence failure'); END;",
            )
            .unwrap();
        drop(store);

        let unit = fixture16_unit_384();
        let filter = meta_of(&[("seq", "03"), ("group", "fixture16")]);
        let err = super::run_with_model_embed(
            &root,
            "secret query text",
            &filter,
            || Ok(dummy_paths()),
            move |_paths, _text| Ok(unit.clone()),
        )
        .expect_err("persistence must fail");
        assert_eq!(err.to_string(), "i/o error: query failed");
        assert!(!err.to_string().contains("secret"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn q73_canonical_filter_and_metadata_byte_order_on_success_path() {
        // Same real-index success path, but with the query embedding equal to
        // the stored unit vectors so rows qualify; filter keys are supplied
        // out of order and must render canonically in byte order, as must
        // per-hit metadata.
        let root = fixture16_seed_real_index();
        let unit = fixture16_unit_384();
        let filter = meta_of(&[("seq", "03"), ("group", "fixture16")]);
        let unit2 = unit.clone();
        let out = super::run_with_model_embed(
            &root,
            "hello",
            &filter,
            || Ok(dummy_paths()),
            move |_paths, _text| Ok(unit2.clone()),
        )
        .expect("filtered success");
        let value: serde_json::Value = serde_json::from_str(&out.body).expect("valid query JSON");
        assert_eq!(value["query"], "hello");
        assert_eq!(
            value["filter"],
            serde_json::json!({"group":"fixture16", "seq":"03"})
        );
        assert_eq!(value["hits"][0]["id"], 4);
        assert_eq!(
            value["hits"][0]["metadata"],
            serde_json::json!({"group":"fixture16", "seq":"03"})
        );
        assert_eq!(value["hits"][0]["link"]["fact_id"], 4);
        assert_eq!(value["hits"][0]["link"]["query_id"], 1);
        assert!(value["hits"][0]["link"]["created_at"].as_i64().unwrap() > 0);
        // Byte-order positions: filter `group` before `seq`, hit `metadata`
        // likewise, top-level query < filter < max_distance < hits.
        let fi = out.body.find("\"filter\"").unwrap();
        let fseg = &out.body[fi..];
        assert!(fseg.find("\"group\"").unwrap() < fseg.find("\"seq\"").unwrap());
        let mi = out.body.find("\"metadata\"").unwrap();
        let mseg = &out.body[mi..];
        assert!(mseg.find("\"group\"").unwrap() < mseg.find("\"seq\"").unwrap());
        let qi = out.body.find("\"query\"").unwrap();
        let mxi = out.body.find("\"max_distance\"").unwrap();
        let hi = out.body.find("\"hits\"").unwrap();
        assert!(qi < fi && fi < mxi && mxi < hi);
        q73_assert_no_leaks(&out.body, &root);
        let _ = std::fs::remove_dir_all(&root);
    }
}
