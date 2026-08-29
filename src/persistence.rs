//! Single canonical-file persistence boundary (design D4, requirement: single
//! atomic persistence boundary).
//!
//! This module is the **only** writer of canonical Markdown. It owns `init`
//! (create dir/file when absent) and the atomic temp-file + rename writer. No
//! other module writes canonical Markdown; the analyzer and external callers
//! have no canonical-write path.

use crate::document_parser;
use crate::error::LibraryError;
use std::fs;
use std::path::{Path, PathBuf};

/// Canonical storage directory, relative to the project root.
pub const CANONICAL_DIR: &str = ".infraware";
/// Canonical document file name.
pub const CANONICAL_FILE: &str = "ARCHITECTURE.md";

/// Path to the canonical document inside `root`.
pub fn canonical_path(root: &Path) -> PathBuf {
    root.join(CANONICAL_DIR).join(CANONICAL_FILE)
}

/// Initialize canonical storage: create the directory and file only when
/// absent. Writes Markdown only (never YAML/JSON). Idempotent.
///
/// Never opens an existing file in truncate or append mode; if the canonical
/// file already exists, its content is left untouched.
pub(crate) fn init(root: &Path, content: &str) -> Result<(), LibraryError> {
    let dir = root.join(CANONICAL_DIR);
    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|e| LibraryError::Io {
            message: format!("failed to create {CANONICAL_DIR}: {e}"),
        })?;
    }
    let file = dir.join(CANONICAL_FILE);
    if !file.exists() {
        // Create new only; never open an existing file in truncate/append mode.
        fs::write(&file, content).map_err(|e| LibraryError::Io {
            message: format!("failed to write {CANONICAL_FILE}: {e}"),
        })?;
    }
    Ok(())
}

/// Write the complete canonical document atomically.
///
/// Validates the complete new content (it must parse as a canonical document)
/// before writing, writes a temporary file in the target directory, renames it
/// into place, and removes the temporary file on write or rename failure. On
/// failure the prior canonical content is left untouched with no leftover
/// temporary artifact. Stores Markdown only.
pub fn write_atomic(root: &Path, content: &str) -> Result<(), LibraryError> {
    // Validate-before-write: never persist malformed canonical content.
    document_parser::parse_document(content).map_err(|e| LibraryError::MalformedDocument {
        message: format!("refusing to write malformed canonical document: {e}"),
    })?;

    let target = canonical_path(root);
    let Some(parent) = target.parent() else {
        return Err(LibraryError::Io {
            message: "canonical path has no parent directory".to_string(),
        });
    };

    // Temporary file lives in the same directory so the rename is atomic on the
    // same filesystem.
    let tmp_name = format!(".{CANONICAL_FILE}.tmp.{}", std::process::id());
    let tmp = parent.join(tmp_name);

    // Write the temporary file; clean it up on failure.
    if let Err(e) = fs::write(&tmp, content) {
        let _ = fs::remove_file(&tmp);
        return Err(LibraryError::Io {
            message: format!("failed to write temporary canonical file: {e}"),
        });
    }

    // Rename into place; clean up the temp file on failure.
    if let Err(e) = fs::rename(&tmp, &target) {
        let _ = fs::remove_file(&tmp);
        return Err(LibraryError::Io {
            message: format!("failed to rename temporary canonical file: {e}"),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_path_joins_dir_and_file() {
        let root = Path::new("/tmp/proj");
        assert_eq!(
            canonical_path(root),
            PathBuf::from("/tmp/proj/.infraware/ARCHITECTURE.md")
        );
    }
}
