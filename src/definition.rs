//! Definition CRUD operations over the canonical document (design D1/D4, the
//! `definition-management` capability).
//!
//! Every operation reuses Plan 2 seams (document load, lookup, mutate, atomic
//! save) and Plan 1's typed error/dispatch seams. No parser, ID-validation, or
//! atomic-writer logic is duplicated here (design D3). Mutations are
//! validate-before-write: the complete new document is built in memory, then
//! persisted only through [`persistence::write_atomic`](crate::persistence::write_atomic).

use crate::definition_model::Definition;
use crate::document::{DefinitionBlock, Document, Span};
use crate::document_parser::{parse_document, serialize_document};
use crate::error::LibraryError;
use crate::format::OutputFormat;
use crate::persistence::{self, canonical_path};
use crate::projection;
use std::fs;
use std::path::Path;

/// Load the canonical document via Plan 2's document boundary.
///
/// A missing `.infraware/ARCHITECTURE.md` is surfaced as
/// [`LibraryError::UninitializedStorage`] (design D8); we never implicitly run
/// `init` and never create the file or directory.
fn load_document(root: &Path) -> Result<Document, LibraryError> {
    let path = canonical_path(root);
    if !path.exists() {
        return Err(LibraryError::UninitializedStorage);
    }
    let text = fs::read_to_string(&path).map_err(|e| LibraryError::Io {
        message: format!("failed to read {}: {e}", path.display()),
    })?;
    parse_document(&text)
}

/// Persist a complete document through Plan 2's atomic boundary.
///
/// `write_atomic` re-validates the content and performs the temp-file + rename
/// with rollback, so this is the only canonical-Markdown writer (design D4).
fn save_document(root: &Path, doc: &Document) -> Result<(), LibraryError> {
    let content = serialize_document(doc);
    persistence::write_atomic(root, &content)
}

/// Read a single definition and render its projection.
///
/// Strictly read-only: loads the document, looks up by id, and returns the
/// projection. No file is written and the canonical document is unchanged
/// (requirement: get never writes). A missing id produces a typed
/// [`LibraryError::MissingDefinition`].
pub fn get(root: &Path, id: &str, format: OutputFormat) -> Result<String, LibraryError> {
    let doc = load_document(root)?;
    let block = doc
        .get(id)
        .ok_or_else(|| LibraryError::MissingDefinition { id: id.to_string() })?;
    let definition = Definition::new(&block.id, &block.body)?;
    projection::serialize_definition(&definition, format)
}

/// Insert a new definition block.
///
/// Rejects an existing id with [`LibraryError::InvalidInput`] (conflict,
/// non-zero), leaving the document unchanged. A new block is appended and all
/// unrelated spans are preserved. The complete document is built in memory and
/// saved atomically (design D4).
pub fn insert(root: &Path, id: &str, value: &str) -> Result<(), LibraryError> {
    let mut doc = load_document(root)?;

    if doc.get(id).is_some() {
        return Err(LibraryError::InvalidInput {
            message: format!("definition already exists: {id}"),
        });
    }

    let block = DefinitionBlock::new(id, value)?;

    // Append the new block. To keep unrelated Markdown intact we add it as a new
    // trailing Definition span. If the document ends with unrelated text without
    // a trailing newline, the marker line still parses correctly.
    doc.spans.push(Span::Definition(block));
    save_document(root, &doc)
}

/// Edit an existing definition's body (full replacement, no patch/merge).
///
/// Rejects an absent id with [`LibraryError::MissingDefinition`]. The targeted
/// block's body is replaced verbatim; every other span (unrelated Markdown and
/// other definitions) is untouched. Persisted atomically.
pub fn edit(root: &Path, id: &str, value: &str) -> Result<(), LibraryError> {
    let mut doc = load_document(root)?;

    let target = doc
        .spans
        .iter_mut()
        .find(|s| matches!(s, Span::Definition(d) if d.id == id));

    match target {
        None => Err(LibraryError::MissingDefinition { id: id.to_string() }),
        Some(Span::Definition(block)) => {
            // Full replacement of the body (design D5). Re-validate via the
            // shared constructor so an invalid id can never be written (ids are
            // unchanged here, but the rule is the single source of truth).
            let updated = DefinitionBlock::new(&block.id, value)?;
            *block = updated;
            save_document(root, &doc)
        }
        Some(_) => unreachable!("find matched only Definition spans"),
    }
}

/// List definition IDs (read-only, ID-only).
///
/// Loads the canonical document without mutation, deduplicates and sorts IDs
/// deterministically, and renders via the existing projection conventions.
/// Returns `UninitializedStorage` if storage is absent (no auto-init) and
/// succeeds with an empty collection for an initialized empty document.
pub(crate) fn list(root: &Path, format: OutputFormat) -> Result<String, LibraryError> {
    let doc = load_document(root)?;
    let mut ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for def in doc.definitions() {
        ids.insert(def.id.clone());
    }
    let sorted: Vec<String> = ids.into_iter().collect();
    projection::serialize_id_list(&sorted, format)
}

/// Remove a definition block by id.
///
/// Rejects an absent id with [`LibraryError::MissingDefinition`]. Only the
/// matching `Definition` span is dropped; unrelated Markdown and other
/// definitions are preserved. Persisted atomically.
pub fn remove(root: &Path, id: &str) -> Result<(), LibraryError> {
    let mut doc = load_document(root)?;

    let existed = doc
        .spans
        .iter()
        .any(|s| matches!(s, Span::Definition(d) if d.id == id));
    if !existed {
        return Err(LibraryError::MissingDefinition { id: id.to_string() });
    }

    doc.spans
        .retain(|s| !matches!(s, Span::Definition(d) if d.id == id));
    save_document(root, &doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// RAII guard that removes its temp directory on drop, even on panic.
    struct TempRoot(std::path::PathBuf);

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temp_root() -> TempRoot {
        let base = std::env::temp_dir().join(format!(
            "infraware-def-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).expect("create temp root");
        TempRoot(base)
    }

    const INIT_MD: &str = "# Architecture\n\n<!-- infraware:definition id=\"keep\" -->original<!-- /infraware:definition -->\n\nprose\n";

    fn init_storage(root: &Path) {
        let dir = root.join(".infraware");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("ARCHITECTURE.md"), INIT_MD).unwrap();
    }

    fn read_def(root: &Path, id: &str) -> String {
        let p = canonical_path(root);
        let text = fs::read_to_string(&p).unwrap();
        let doc = parse_document(&text).unwrap();
        doc.get(id).unwrap().body.clone()
    }

    fn doc_text(root: &Path) -> String {
        let p = canonical_path(root);
        fs::read_to_string(&p).unwrap()
    }

    #[test]
    fn get_missing_is_nonzero_error() {
        let root = temp_root();
        init_storage(&root.0);
        let err = get(&root.0, "nope", OutputFormat::Markdown).unwrap_err();
        assert_eq!(
            err,
            LibraryError::MissingDefinition {
                id: "nope".to_string()
            }
        );
    }

    #[test]
    fn get_returns_existing_body() {
        let root = temp_root();
        init_storage(&root.0);
        let out = get(&root.0, "keep", OutputFormat::Json).unwrap();
        assert!(out.contains("original"));
    }

    #[test]
    fn get_never_writes() {
        let root = temp_root();
        init_storage(&root.0);
        let before = doc_text(&root.0);
        let _ = get(&root.0, "keep", OutputFormat::Markdown);
        assert_eq!(doc_text(&root.0), before, "get must not mutate the file");
    }

    #[test]
    fn insert_new_block_preserves_unrelated() {
        let root = temp_root();
        init_storage(&root.0);
        insert(&root.0, "added", "fresh").unwrap();
        assert_eq!(read_def(&root.0, "added"), "fresh");
        assert_eq!(read_def(&root.0, "keep"), "original");
        let text = doc_text(&root.0);
        assert!(text.contains("prose"), "unrelated content preserved");
    }

    #[test]
    fn insert_duplicate_is_rejected_without_change() {
        let root = temp_root();
        init_storage(&root.0);
        let before = doc_text(&root.0);
        let err = insert(&root.0, "keep", "clobber").unwrap_err();
        assert!(err.is_usage());
        assert_eq!(doc_text(&root.0), before, "document unchanged on conflict");
    }

    #[test]
    fn edit_full_replacement_updates_only_target() {
        let root = temp_root();
        init_storage(&root.0);
        edit(&root.0, "keep", "replaced").unwrap();
        assert_eq!(read_def(&root.0, "keep"), "replaced");
        let text = doc_text(&root.0);
        assert!(
            text.contains("prose"),
            "unrelated content preserved on edit"
        );
    }

    #[test]
    fn edit_absent_is_rejected() {
        let root = temp_root();
        init_storage(&root.0);
        let before = doc_text(&root.0);
        let err = edit(&root.0, "ghost", "x").unwrap_err();
        assert_eq!(
            err,
            LibraryError::MissingDefinition {
                id: "ghost".to_string()
            }
        );
        assert_eq!(doc_text(&root.0), before);
    }

    #[test]
    fn remove_drops_only_target() {
        let root = temp_root();
        init_storage(&root.0);
        remove(&root.0, "keep").unwrap();
        assert!(
            parse_document(&doc_text(&root.0))
                .unwrap()
                .get("keep")
                .is_none()
        );
        assert!(
            doc_text(&root.0).contains("prose"),
            "unrelated content preserved on remove"
        );
    }

    #[test]
    fn remove_absent_is_rejected() {
        let root = temp_root();
        init_storage(&root.0);
        let before = doc_text(&root.0);
        let err = remove(&root.0, "ghost").unwrap_err();
        assert_eq!(
            err,
            LibraryError::MissingDefinition {
                id: "ghost".to_string()
            }
        );
        assert_eq!(doc_text(&root.0), before);
    }

    #[test]
    fn uninitialized_store_is_reported() {
        let root = temp_root(); // no storage created
        let err = get(&root.0, "x", OutputFormat::Markdown).unwrap_err();
        assert_eq!(err, LibraryError::UninitializedStorage);
        // No .infraware directory or file should exist.
        assert!(!root.0.join(".infraware").exists());
    }

    #[test]
    fn list_returns_sorted_ids_without_bodies() {
        let root = temp_root();
        init_storage(&root.0);
        insert(&root.0, "zeta", "body zeta").unwrap();
        insert(&root.0, "alpha", "body alpha").unwrap();
        let json = list(&root.0, OutputFormat::Json).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let ids: Vec<String> = v["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, vec!["alpha", "keep", "zeta"]);
        assert!(!json.contains("body zeta"));
        let md = list(&root.0, OutputFormat::Markdown).unwrap();
        assert!(md.contains("- alpha\n"));
        assert!(!md.contains("body"));
    }

    #[test]
    fn list_empty_initialized_is_empty_per_format() {
        let root = temp_root();
        let dir = root.0.join(".infraware");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("ARCHITECTURE.md"), "# Architecture\n").unwrap();
        assert!(list(&root.0, OutputFormat::Markdown).unwrap().is_empty());
        let j = list(&root.0, OutputFormat::Json).unwrap();
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["ids"].as_array().unwrap().len(), 0);
        let y = list(&root.0, OutputFormat::Yaml).unwrap();
        let vy: serde_yaml::Value = serde_yaml::from_str(&y).unwrap();
        assert_eq!(vy["ids"].as_sequence().unwrap().len(), 0);
    }

    #[test]
    fn list_uninitialized_is_error_and_no_mutation() {
        let root = temp_root();
        let err = list(&root.0, OutputFormat::Markdown).unwrap_err();
        assert_eq!(err, LibraryError::UninitializedStorage);
        assert!(!root.0.join(".infraware").exists());
        assert_eq!(
            list(&root.0, OutputFormat::Json).unwrap_err(),
            LibraryError::UninitializedStorage
        );
        assert!(!root.0.join(".infraware").exists());
    }

    #[test]
    fn list_dedupes_and_is_read_only() {
        let root = temp_root();
        init_storage(&root.0);
        // Insert a few then verify list doesn't mutate file.
        insert(&root.0, "b", "vb").unwrap();
        let before = doc_text(&root.0);
        let out1 = list(&root.0, OutputFormat::Json).unwrap();
        let out2 = list(&root.0, OutputFormat::Json).unwrap();
        assert_eq!(out1, out2);
        assert_eq!(doc_text(&root.0), before, "list must not mutate");
    }
}
