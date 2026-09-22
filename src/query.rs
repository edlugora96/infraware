//! Read-only text-rule execution seam.
//!
//! This module is the dispatch target for [`Command::Query`](crate::Command).
//! It validates the read-only store, obtains a cached model embedding, ranks
//! stored vectors, and renders the compact text-only success envelope.

use std::path::Path;

use serde::Serialize;

use crate::dispatch::DispatchOutput;
use crate::error::LibraryError;

/// Single ranked hit in the success envelope.
///
/// Field declaration order is the serialization order: `id`, `score`, `distance`, `text`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct QueryHit {
    pub(crate) id: i64,
    pub(crate) score: f64,
    pub(crate) distance: f64,
    pub(crate) text: String,
}

/// Success envelope for `query`.
///
/// Field declaration order is the serialization order: `task`, `max_distance`, `hits`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct QueryEnvelope {
    pub(crate) task: String,
    pub(crate) max_distance: f64,
    pub(crate) hits: Vec<QueryHit>,
}

/// Render the exact compact success JSON.
///
/// - Preserves `task` text bytes exactly (no trim/normalize).
/// - Compact form only (`serde_json::to_string`); empty `hits` renders `[]`.
/// - No extra fields or alternate output modes.
///
pub(crate) fn render_envelope(task: &str, hits: &[QueryHit], max_distance: f64) -> String {
    let envelope = QueryEnvelope {
        task: task.to_string(),
        max_distance,
        hits: hits.to_vec(),
    };
    // Serialization of owned strings/floats/maps cannot fail; panic would
    // indicate a programming error, but fall back to a minimal envelope
    // rather than introducing a new error path at the rule layer.
    serde_json::to_string(&envelope).expect("query envelope serialization must succeed")
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

/// Small reader seam for the deep rule path (tasks 6.1-6.2).
///
/// The canonical `filter` is applied against the `meta_kv` side table
/// before any rescore/ranking work: empty means all rows, non-empty means
/// exact string-equality AND over every key/value pair. Production adapter:
/// [`SqliteStore`](crate::vec_store::SqliteStore) filtered fullscan read.
/// Fake adapters return canned `(rowid, embedding, metadata)` rows and
/// apply the same AND semantics in Rust so rescore and later ranking logic
/// stay testable without a DB or model.
#[allow(dead_code)]
pub(crate) trait EmbeddingReader {
    fn read_embeddings(&self) -> Result<Vec<(i64, Vec<f32>)>, LibraryError>;
}

impl EmbeddingReader for crate::vec_store::SqliteStore {
    fn read_embeddings(&self) -> Result<Vec<(i64, Vec<f32>)>, LibraryError> {
        self.scan_filtered_embeddings()
    }
}

/// Exact cosine distance `1 - dot(a,b)/(|a|*|b|)` in f64.
///
/// Zero-norm inputs map to distance 1.0 (orthogonal fallback) so rescore
/// never divides by zero; stored and rule embeddings are normalized, so
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
/// The store holds task vectors only; the rule embedding is produced per
/// call by the model. Tagging each operand at the seam makes the
/// same-type rule (`tasks-to-tasks`, `rules-to-tasks`, never
/// `rule-vs-rule`) checkable instead of merely conventional.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum EmbeddingKind {
    Rule,
    Task,
}

/// Task 2.3: a borrowed embedding slice tagged with its entity type.
///
/// Borrowed (`&[f32]`, never owned/`Clone`) per the lending convention:
/// the tag travels with the borrow and no vector bytes are copied.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub(crate) struct TypedEmbedding<'a> {
    kind: EmbeddingKind,
    values: &'a [f32],
}

#[allow(dead_code)]
impl<'a> TypedEmbedding<'a> {
    pub(crate) fn rule(values: &'a [f32]) -> Self {
        Self {
            kind: EmbeddingKind::Rule,
            values,
        }
    }

    pub(crate) fn task(values: &'a [f32]) -> Self {
        Self {
            kind: EmbeddingKind::Task,
            values,
        }
    }
}

/// Task 2.3: same-type-gated cosine distance.
///
/// `Rule`-vs-`Task` (either order) and `Task`-vs-`Task` compute the exact
/// [`cosine_distance`]; `Rule`-vs-`Rule` is rejected as an operational
/// error (never usage: a cross-type comparison is an internal invariant
/// violation, so it must map to status 1, never status 2). The message is
/// a static token with no paths, SQL, or vector contents.
#[allow(dead_code)]
pub(crate) fn typed_cosine_distance(
    a: &TypedEmbedding<'_>,
    b: &TypedEmbedding<'_>,
) -> Result<f64, LibraryError> {
    match (a.kind, b.kind) {
        (EmbeddingKind::Rule, EmbeddingKind::Rule) => Err(LibraryError::Io {
            message: "cross-type embedding comparison rejected".to_string(),
        }),
        _ => Ok(cosine_distance(a.values, b.values)),
    }
}

/// Rescore every scanned row against the rule embedding in Rust.
///
/// Returns all rows as [`ScoredRow`] sorted by distance ascending then id
/// ascending (same deterministic order as the final ranking seam). No
/// filtering, thresholding, or output here: tasks 6.2/6.3 own those.
///
/// Task 2.3: every comparison runs through [`typed_cosine_distance`] with
/// the live rule tagged `Rule` and each scanned row tagged `Task`, so a
/// rule-vs-rule comparison cannot reach the math by construction (rows
/// come only from [`EmbeddingReader`], which reads stored task vectors).
#[allow(dead_code)]
pub(crate) fn rescore(query_embedding: &[f32], rows: &[(i64, Vec<f32>)]) -> Vec<ScoredRow> {
    let rule = TypedEmbedding::rule(query_embedding);
    let mut out: Vec<ScoredRow> = rows
        .iter()
        .map(|(id, emb)| ScoredRow {
            id: *id,
            distance: typed_cosine_distance(&rule, &TypedEmbedding::task(emb))
                // Infallible by construction: the only pairing built here is
                // rule-vs-task, which the gate always accepts.
                .expect("rule-vs-task pairing is always valid"),
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
#[allow(dead_code)]
pub(crate) fn retrieve_scored(
    reader: &impl EmbeddingReader,
    query_embedding: &[f32],
) -> Result<Vec<ScoredRow>, LibraryError> {
    let rows = reader.read_embeddings()?;
    Ok(rescore(query_embedding, &rows))
}

/// Generic operational failure surfaced for every rule storage error.
///
/// Deliberately fixed and detail-free: no filesystem paths, SQL, vectors,
/// model/cache details, or internal error text. `run` renders it as
/// `error: i/o error: rule failed` (one stderr line, empty stdout).
pub(crate) const QUERY_OPERATIONAL_MESSAGE: &str = "query failed";

fn operational_err() -> LibraryError {
    LibraryError::Io {
        message: QUERY_OPERATIONAL_MESSAGE.to_string(),
    }
}

/// Ordered rule setup seam (task 5.2).
///
/// Guarantees the exact pre-retrieval order on the production path:
/// 1. open the read-only snapshot index,
/// 2. verify schema plus all six `model_meta` values,
/// 3. validate the cached-only model files (no fetcher, no network),
/// 4. embed the byte-exact rule text.
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
    resolve: impl FnOnce() -> Result<crate::model_cache::ModelPaths, LibraryError>,
    embed: impl FnOnce(&crate::model_cache::ModelPaths, &str) -> Result<Vec<f32>, LibraryError>,
    max_distance: f64,
    query_auto_ingest: bool,
) -> Result<DispatchOutput, LibraryError> {
    let store = match crate::vec_store::SqliteStore::open_read_only(root) {
        Ok(store) => store,
        Err(LibraryError::UninitializedStorage) => return Err(LibraryError::UninitializedStorage),
        Err(error) => return Err(error),
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
    let mut rows = match store.historical_query_candidates() {
        Ok(rows) => rows,
        Err(_) => return Err(operational_err()),
    };
    rows.sort_by(|a, b| {
        cosine_distance(&query_embedding, &a.3)
            .total_cmp(&cosine_distance(&query_embedding, &b.3))
            .then_with(|| a.0.cmp(&b.0))
    });
    let mut best = std::collections::BTreeMap::new();
    for crate::vec_store::QueryCandidateRow(rule_id, task_id, created, embedding, rule_text) in rows
    {
        let d = cosine_distance(&query_embedding, &embedding);
        if d <= max_distance {
            let replace = best
                .get(&rule_id)
                .is_none_or(|x: &(f64, i64, i64, String)| d < x.0 || (d == x.0 && task_id < x.1));
            if replace {
                best.insert(rule_id, (d, task_id, created, rule_text));
            }
        }
    }
    let mut ranked: Vec<_> = best.into_iter().collect();
    ranked.sort_by(|a, b| a.1.0.total_cmp(&b.1.0).then_with(|| a.0.cmp(&b.0)));
    let hits = ranked
        .into_iter()
        .map(|(rule_id, (d, _task_id, _created, rule_text))| QueryHit {
            id: rule_id,
            score: 1.0 - d,
            distance: d,
            text: rule_text,
        })
        .collect::<Vec<_>>();
    if hits.is_empty() {
        let body = render_envelope(text, &hits, max_distance);
        return Ok(DispatchOutput {
            format: crate::format::OutputFormat::Json,
            body,
        });
    }

    if !query_auto_ingest {
        let body = render_envelope(text, &hits, max_distance);
        return Ok(DispatchOutput {
            format: crate::format::OutputFormat::Json,
            body,
        });
    }

    // Retrieval is complete before opening the separate write connection.
    // The snapshot and its SQL borrows are dropped before the RW transaction.
    let top_rule_id = hits[0].id;
    drop(store);
    let mut rw = match crate::vec_store::SqliteStore::open_existing_rw(root) {
        Ok(store) => store,
        Err(LibraryError::UninitializedStorage) => return Err(LibraryError::UninitializedStorage),
        Err(_) => return Err(operational_err()),
    };
    if rw
        .insert_task_for_rule(text, &query_embedding, top_rule_id)
        .is_err()
    {
        return Err(operational_err());
    }
    let body = render_envelope(text, &hits, max_distance);
    Ok(DispatchOutput {
        format: crate::format::OutputFormat::Json,
        body,
    })
}

/// Temporary rule execution entry point (tasks 4.3 + 5.2 ordering).
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
    query_max: f64,
    query_auto_ingest: bool,
) -> Result<DispatchOutput, LibraryError> {
    run_with_model_embed(
        root,
        text,
        crate::model_cache::cached_model_files,
        |paths, task_text| {
            let embedder = crate::embed::BgeEmbedder::load(paths).map_err(|_| operational_err())?;
            use crate::ingest::Embedder;
            embedder.embed(task_text).map_err(|_| operational_err())
        },
        query_max,
        query_auto_ingest,
    )
}
