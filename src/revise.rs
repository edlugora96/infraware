//! Timestamp-only `revise` execution.

use std::path::Path;

use crate::command::Entity;
use crate::dispatch::DispatchOutput;
use crate::error::LibraryError;
use crate::format::OutputFormat;

/// Touch an existing row's `updated_at` without changing any other state.
pub(crate) fn run(
    root: &Path,
    id_raw: &str,
    entity: Entity,
) -> Result<DispatchOutput, LibraryError> {
    let id = crate::remove::parse_record_id(id_raw).map_err(|_| LibraryError::InvalidInput {
        message: format!("revise requires a positive integer <ID>, found: {id_raw}"),
    })?;
    let mut store =
        crate::vec_store::SqliteStore::open_existing_rw(root).map_err(|_| LibraryError::Io {
            message: "revise failed".into(),
        })?;
    store.revise(entity, id)?;
    Ok(DispatchOutput {
        format: OutputFormat::Json,
        body: format!(r#"{{"id":{id},"revised":true}}"#),
    })
}
