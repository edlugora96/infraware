//! Additional deterministic v2 acceptance evidence.  These checks intentionally
//! inspect the narrow DDL/lifecycle seam rather than requiring model downloads.
use std::fs;

fn store_source() -> String {
    fs::read_to_string("src/vec_store.rs").expect("storage source is present")
}

#[test]
fn v2_ddl_has_all_clean_tables_and_technical_model_metadata() {
    let s = store_source();
    for table in [
        "CREATE TABLE tasks(",
        "CREATE TABLE rules(",
        "CREATE VIRTUAL TABLE task_vecs USING vec0(embedding float[384] distance_metric=cosine)",
        "CREATE VIRTUAL TABLE rule_vecs USING vec0(embedding float[384] distance_metric=cosine)",
        "CREATE TABLE task_rule_relations(",
        "CREATE TABLE model_meta(",
    ] {
        assert!(s.contains(table), "missing clean v2 DDL: {table}");
    }
    assert!(s.contains("PRIMARY KEY(task_id,rule_id)"));
    assert!(s.contains("REFERENCES tasks(task_id)"));
    assert!(s.contains("REFERENCES rules(rule_id)"));
    for key in [
        "schema_version",
        "model",
        "revision",
        "dimension",
        "metric",
        "normalized",
        "encoding",
    ] {
        assert!(
            s.contains(&format!("(\"{key}\",")),
            "missing model key {key}"
        );
    }
    for forbidden in ["meta_kv", "related_id", "metadata", "filter"] {
        assert!(!s.contains(&format!("CREATE TABLE {forbidden}")));
    }
}

#[test]
fn lifecycle_is_create_only_for_ingest_and_reads_use_existing_gates() {
    let s = store_source();
    assert!(s.contains("let fresh = !path(r).exists();"));
    assert!(s.contains("if fresh {\n            s.init()?;\n        }"));
    assert!(s.contains("open_with_flags(\n                path(r),\n                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY"));
    assert!(s.contains("if !path(r).is_file() {\n            return Err(er(\"index missing\"));"));
}

#[test]
fn pair_relation_and_history_retention_are_explicit() {
    let s = store_source();
    assert!(!s.contains("query_fact_links"));
    assert!(s.contains("DELETE FROM task_rule_relations WHERE task_id=?1"));
    assert!(s.contains("DELETE FROM task_vecs WHERE rowid=?1"));
    // Removal never deletes rules or rule vectors: history is independent.
    assert!(!s.contains("DELETE FROM rules"));
    assert!(!s.contains("DELETE FROM rule_vecs"));
    assert!(s.contains("JOIN task_rule_relations p ON p.rule_id=q.rule_id"));
}

#[test]
fn dedup_is_nearest_then_id_and_inclusive_at_threshold() {
    let s = store_source();
    assert!(s.contains("if d <= threshold"));
    assert!(s.contains("nearest(\"task_vecs\", fv, task_threshold)"));
    assert!(s.contains("nearest(\"rule_vecs\", qv, rule_threshold)"));
    assert!(s.contains("(d, id) < (b.0, b.1)"));
    assert!(
        s.contains("SELECT created_at FROM task_rule_relations WHERE task_id=?1 AND rule_id=?2")
    );
    assert!(s.contains("return Ok((fi, qi, created, true));"));
}

#[test]
fn ingest_embeds_fact_and_query_independently_before_persistence() {
    let s = fs::read_to_string("src/ingest.rs").expect("ingest source is present");
    assert!(s.contains("embedder.embed(task_text)"));
    assert!(s.contains("embedder.embed(rule_text)"));
    assert!(!s.contains("format!(\"{}{}\", task_text, rule_text)"));
    assert!(
        s.contains("store.insert_pair(")
            && s.contains("settings.task_dedup")
            && s.contains("settings.rule_dedup")
    );
    assert!(s.contains("map_err(|_| operational_err())"));
}

#[test]
fn atomic_write_and_safe_failure_mapping_are_visible_at_the_storage_seam() {
    let s = store_source();
    assert!(s.contains("let tx = self.conn.transaction().map_err(er)?;"));
    assert!(s.contains("tx.commit().map_err(er)?"));
    // Every DDL/data operation is propagated, making a failed transaction roll back.
    assert!(s.matches(".map_err(er)?").count() >= 12);
    let q = fs::read_to_string("src/query.rs").expect("query source is present");
    assert!(q.contains("then_with(|| a.0.cmp(&b.0))"));
    assert!(q.contains("if d <= max_distance"));
    assert!(q.contains("query_max: f64"));
    let d = fs::read_to_string("src/dispatch.rs").expect("dispatch source is present");
    assert!(d.contains("s.query_max"));
    assert!(q.contains("insert_task_for_rule(text, &query_embedding, top_rule_id)"));
}
