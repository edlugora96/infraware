//! semantic search `patch` execution seam.
//!
//! Grammar: `patch <positive-integer-id> --text <text>`.
//! The public ID stays stable while its text and vector are replaced.
//!
//! Ordering: validate arguments, open storage, load the record before model
//! work, then embed changed text outside the write transaction and apply one
//! atomic update. An identical patch is a successful no-op (`patched:false`).

use std::path::Path;

use crate::command::Entity;
use crate::dispatch::DispatchOutput;
use crate::error::LibraryError;
use crate::format::OutputFormat;

/// Generic operational failure for the patch path.
///
/// Detail-free like the rule/ingest/remove gates: storage paths, SQL,
/// vectors, and model/cache details never reach diagnostics. Non-usage, so
/// the adapter reports status 1 with empty stdout. Usage errors and the
/// distinct record-not-found error pass through unchanged.
fn operational_err() -> LibraryError {
    LibraryError::Io {
        message: "patch failed".to_string(),
    }
}

/// Mutation-store seam for patch orchestration.
///
/// Production adapter is [`SqliteStore`](crate::vec_store::SqliteStore);
/// in-memory fakes prove ordering and zero embeds for identical patches without
/// SQLite or a model.
pub(crate) trait MutationStore {
    fn read_entity_text(&self, entity: Entity, id: i64) -> Result<String, LibraryError>;
    fn apply_patch(
        &mut self,
        entity: Entity,
        id: i64,
        text: &str,
        embedding: Option<&[f32]>,
        threshold: f64,
    ) -> Result<bool, LibraryError>;
}
impl MutationStore for crate::vec_store::SqliteStore {
    fn read_entity_text(&self, entity: Entity, id: i64) -> Result<String, LibraryError> {
        crate::vec_store::SqliteStore::read_entity_text(self, entity, id)
    }
    fn apply_patch(
        &mut self,
        entity: Entity,
        id: i64,
        text: &str,
        embedding: Option<&[f32]>,
        threshold: f64,
    ) -> Result<bool, LibraryError> {
        crate::vec_store::SqliteStore::apply_patch(self, entity, id, text, embedding, threshold)
    }
}

/// Successful patch outcome: the stable public ID plus whether anything changed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PatchOutcome {
    pub(crate) id: i64,
    pub(crate) patched: bool,
    pub(crate) duplicate: bool,
}

/// Render the exact compact success JSON: `{"id":N,"patched":true|false}`.
fn output_json(out: &PatchOutcome) -> String {
    format!(
        "{{\"id\":{},\"patched\":{},\"duplicate\":{}}}",
        out.id, out.patched, out.duplicate
    )
}

struct ValidatedPatch {
    id: i64,
    text: String,
    entity: Entity,
}
fn validate_patch_args(
    id_raw: &str,
    text: Option<&str>,
    entity: Entity,
    _unsupported: Option<&str>,
) -> Result<ValidatedPatch, LibraryError> {
    let id = crate::remove::parse_record_id(id_raw).map_err(|_| LibraryError::InvalidInput {
        message: format!("patch requires a positive integer <ID>, found: {id_raw}"),
    })?;
    let Some(text) = text else {
        return Err(LibraryError::InvalidInput {
            message: "patch requires --text".into(),
        });
    };
    if _unsupported.is_some() {
        return Err(LibraryError::InvalidInput {
            message: "unknown patch option".into(),
        });
    }
    if text.trim().is_empty() {
        return Err(LibraryError::InvalidInput {
            message: "patch text must not be empty".into(),
        });
    }
    Ok(ValidatedPatch {
        id,
        text: text.into(),
        entity,
    })
}
enum PatchPlan {
    NoOp(i64),
    Change(i64, String),
}
fn plan_patch<S: MutationStore>(s: &S, v: &ValidatedPatch) -> Result<PatchPlan, LibraryError> {
    let old = s.read_entity_text(v.entity, v.id)?;
    if old == v.text {
        Ok(PatchPlan::NoOp(v.id))
    } else {
        Ok(PatchPlan::Change(v.id, v.text.clone()))
    }
}
fn execute_patch<S: MutationStore>(
    s: &mut S,
    v: &ValidatedPatch,
    embed: impl FnOnce(&str) -> Result<Vec<f32>, LibraryError>,
    threshold: f64,
) -> Result<PatchOutcome, LibraryError> {
    match plan_patch(s, v)? {
        PatchPlan::NoOp(id) => Ok(PatchOutcome {
            id,
            patched: false,
            duplicate: false,
        }),
        PatchPlan::Change(id, text) => {
            let e = embed(&text).map_err(|x| if x.is_usage() { x } else { operational_err() })?;
            let duplicate = match s.apply_patch(v.entity, id, &text, Some(&e), threshold) {
                Ok(value) => value,
                Err(LibraryError::MissingDefinition { .. }) => {
                    return Err(LibraryError::MissingDefinition { id: id.to_string() });
                }
                Err(_) => return Err(operational_err()),
            };
            Ok(PatchOutcome {
                id,
                patched: true,
                duplicate,
            })
        }
    }
}

/// Production `patch`: pure validation first, then read-write open without
/// creation (missing storage is operational), then plan/execute. The model
/// cache is ensured lazily inside the embed closure, so identical patches
/// perform zero model work.
pub(crate) fn run(
    root: &Path,
    id_raw: &str,
    text: Option<&str>,
    entity: Entity,
) -> Result<DispatchOutput, LibraryError> {
    // Validate argv/payload before opening storage or touching the model.
    let validated = validate_patch_args(id_raw, text, entity, None)?;
    let settings = crate::settings::load(root)?;
    let threshold = match entity {
        Entity::Rules => settings.rule_dedup,
        Entity::Tasks => settings.task_dedup,
    };
    let mut store = match crate::vec_store::SqliteStore::open_existing_rw(root) {
        Ok(store) => store,
        Err(_) => return Err(operational_err()),
    };
    let outcome = execute_patch(
        &mut store,
        &validated,
        |t| {
            let paths = crate::model_cache::cached_model_files().map_err(|_| operational_err())?;
            let embedder =
                crate::embed::BgeEmbedder::load(&paths).map_err(|_| operational_err())?;
            use crate::ingest::Embedder;
            match embedder.embed(t) {
                Ok(embedding) => Ok(embedding),
                Err(e) if e.is_usage() => Err(e),
                Err(_) => Err(operational_err()),
            }
        },
        threshold,
    )?;
    Ok(DispatchOutput {
        format: OutputFormat::Json,
        body: output_json(&outcome),
    })
}
