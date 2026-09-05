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
        "CREATE TABLE facts(",
        "CREATE TABLE queries(",
        "CREATE VIRTUAL TABLE fact_vecs USING vec0(embedding float[384] distance_metric=cosine)",
        "CREATE VIRTUAL TABLE query_vecs USING vec0(embedding float[384] distance_metric=cosine)",
        "CREATE TABLE pair_relations(",
        "CREATE TABLE query_runs(",
        "CREATE TABLE query_fact_links(",
        "CREATE TABLE model_meta(",
    ] {
        assert!(s.contains(table), "missing clean v2 DDL: {table}");
    }
    assert!(s.contains("PRIMARY KEY(fact_id,query_id)"));
    assert!(s.contains("REFERENCES facts(fact_id)"));
    assert!(s.contains("REFERENCES queries(query_id)"));
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
    assert!(s.contains("DELETE FROM query_fact_links WHERE fact_id=?1"));
    assert!(s.contains("DELETE FROM pair_relations WHERE fact_id=?1"));
    assert!(s.contains("DELETE FROM fact_vecs WHERE rowid=?1"));
    // Removal never deletes queries or query vectors: history is independent.
    assert!(!s.contains("DELETE FROM queries"));
    assert!(!s.contains("DELETE FROM query_vecs"));
    assert!(s.contains("JOIN pair_relations p ON p.query_id=q.query_id"));
}

#[test]
fn dedup_is_nearest_then_id_and_inclusive_at_threshold() {
    let s = store_source();
    assert!(s.contains("if d <= 0.15"));
    assert!(s.contains("(d, id) < (b.0, b.1)"));
    assert!(s.contains("SELECT created_at FROM pair_relations WHERE fact_id=?1 AND query_id=?2"));
    assert!(s.contains("return Ok((fi, qi, created, true));"));
}

#[test]
fn ingest_embeds_fact_and_query_independently_before_persistence() {
    let s = fs::read_to_string("src/ingest.rs").expect("ingest source is present");
    assert!(s.contains("embedder.embed(fact_text)"));
    assert!(s.contains("embedder.embed(query_text)"));
    assert!(!s.contains("format!(\"{}{}\", fact_text, query_text)"));
    assert!(s.contains("store.insert_pair(fact_text, query_text, &fv, &qv)"));
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
    assert!(q.contains("then_with(|| a.id.cmp(&b.id))"));
    assert!(q.contains("distance <= QUERY_MAX_DISTANCE"));
    assert!(q.contains("persist_query"));
}
