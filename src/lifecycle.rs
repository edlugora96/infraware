//! Canonical-document lifecycle (design D4, requirement: idempotent init).
//!
//! Builds the initial canonical document (title + guidance comments only) and
//! owns the `init` behavior. All canonical writes funnel through the
//! [`persistence`](crate::persistence) boundary, which is the only module that
//! writes canonical Markdown.

use crate::error::LibraryError;
use crate::guidance;
use crate::persistence;
use std::path::Path;

/// Build the initial canonical document content.
///
/// Contains only a title heading and helpful guidance comments drawn from the
/// static catalog. It creates **no** definitions, answers, or mandatory schema
/// (spec: Init writes comments only).
pub fn init_markdown() -> String {
    let mut doc = String::new();
    doc.push_str("# Architecture\n\n");
    doc.push_str(&guidance::render_comments());
    doc
}

/// Initialize canonical storage for `root`.
///
/// Creates `.infraware/` only when absent and `ARCHITECTURE.md` only when
/// absent. Never truncates or appends to an existing file. Idempotent: repeated
/// calls leave existing content unchanged (spec: idempotent no-clobber init).
pub fn init(root: &Path) -> Result<(), LibraryError> {
    persistence::init(root, &init_markdown())
}
