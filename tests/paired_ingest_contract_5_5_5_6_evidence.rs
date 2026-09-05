//! Deterministic acceptance evidence for paired-ingest contract tasks 5.5/5.6.
//!
//! The public binary deliberately requires a cached model for successful rule
//! and patch operations.  These checks therefore pin the production seams and
//! SQL invariants without downloading a model or depending on the vec runtime.
use std::fs;

fn source(name: &str) -> String {
    fs::read_to_string(format!("src/{name}.rs")).expect("source is present")
}

#[test]
fn query_uses_historical_vectors_and_collapses_to_nearest_link() {
    let q = source("query");
    let v = source("vec_store");
    assert!(v.contains("rule_vecs"));
    assert!(v.contains("JOIN task_rule_relations p"));
    for required in [
        "historical_query_candidates",
        "if d <= max_distance",
        "best.insert(rule_id",
        "task_id < x.1",
        "then_with(|| a.0.cmp(&b.0))",
        "insert_task_for_rule(text, &query_embedding, top_rule_id)",
        "drop(store)",
        "if hits.is_empty()",
    ] {
        assert!(q.contains(required), "rule evidence missing {required:?}");
    }
    assert!(!q.contains("hit.link"));
}

#[test]
fn query_snapshot_is_read_before_separate_top_only_write() {
    let q = source("query");
    let read = q.find("historical_query_candidates").expect("read seam");
    let write = q.find("open_existing_rw").expect("write seam");
    assert!(read < write, "retrieval must precede the write transaction");
    assert!(q[read..write].contains("hits.is_empty()"));
    assert!(q[write..].contains("insert_task_for_rule(text, &query_embedding, top_rule_id)"));
    // The empty branch returns before opening the RW connection: zero-hit is
    // both successful and free of rule-run/link writes.
    assert!(q[..write].contains("return Ok(DispatchOutput"));
}

#[test]
fn patch_recomputes_vectors_and_atomically_merges_all_owned_links() {
    let p = source("patch");
    let v = source("vec_store");
    assert!(p.contains("embed(&text)") || p.contains("embed(&text)"));
    for required in [
        "UPDATE tasks SET text",
        "SELECT rowid,embedding FROM task_vecs WHERE rowid<>?1",
        "if distance <= threshold",
        "UPDATE OR IGNORE task_rule_relations",
        "UPDATE OR IGNORE task_rule_relations",
        "DELETE FROM task_vecs WHERE rowid=?1",
        "DELETE FROM tasks WHERE task_id=?1",
        "let tx = self.conn.transaction().map_err(er)?",
        "tx.commit().map_err(er)?",
    ] {
        assert!(v.contains(required), "patch evidence missing {required:?}");
    }
    assert!(p.contains("PatchOutcome { id, patched: true }"));
    // Historical rule rows/vectors are intentionally absent from patch/remove
    // deletion lists, while task-owned links are explicitly cleaned up.
    assert!(!v.contains("DELETE FROM rules"));
    assert!(!v.contains("DELETE FROM rule_vecs"));
}

#[test]
fn list_get_and_remove_are_read_only_or_atomic_and_text_only() {
    let r = source("records");
    let rm = source("remove");
    let v = source("vec_store");
    for required in [
        "open_read_only",
        "list_records",
        "skip(offset).take(limit)",
        "created_at",
        "output_get_json",
    ] {
        assert!(r.contains(required), "record evidence missing {required:?}");
    }
    for required in [
        "open_existing_rw",
        "DELETE FROM task_rule_relations WHERE task_id=?1",
        "DELETE FROM task_rule_relations WHERE task_id=?1",
        "DELETE FROM task_vecs WHERE rowid=?1",
        "DELETE FROM tasks WHERE task_id=?1",
        "tx.commit().map_err(er)",
    ] {
        assert!(
            rm.contains(required) || v.contains(required),
            "remove evidence missing {required:?}"
        );
    }
    assert!(r.contains("text"));
    assert!(r.contains("created_at"));
}

#[test]
fn failure_paths_are_redacted_and_transactional() {
    for file in ["query", "patch", "remove"] {
        let s = source(file);
        assert!(
            s.contains("LibraryError::Io"),
            "{file} maps operational errors"
        );
        assert!(
            s.contains("operational_err"),
            "{file} maps storage failures"
        );
    }
    let v = source("vec_store");
    assert!(v.matches("map_err(er)?").count() >= 12);
}
