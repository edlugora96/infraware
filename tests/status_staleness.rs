//! Status freshness is based on rules, while task timestamps remain stored data.
use infraware::run;
use rusqlite::Connection;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

fn lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
fn root(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("infraware-status-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}
fn store(p: &Path, task: i64, rule: i64, updated: Option<i64>) {
    let db = p.join(".infraware/vec-index.sqlite");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    let c = Connection::open(db).unwrap();
    c.execute_batch("CREATE TABLE tasks(task_id INTEGER PRIMARY KEY,text TEXT NOT NULL,created_at INTEGER NOT NULL,updated_at INTEGER NULL); CREATE TABLE rules(rule_id INTEGER PRIMARY KEY,text TEXT NOT NULL,created_at INTEGER NOT NULL,updated_at INTEGER NULL); CREATE TABLE task_vecs(rowid INTEGER PRIMARY KEY,embedding BLOB NOT NULL,CHECK(embedding IS NOT 'float[384] cosine')); CREATE TABLE rule_vecs(rowid INTEGER PRIMARY KEY,embedding BLOB NOT NULL,CHECK(embedding IS NOT 'float[384] cosine')); CREATE TABLE task_rule_relations(task_id INTEGER NOT NULL,rule_id INTEGER NOT NULL,created_at INTEGER,PRIMARY KEY(task_id,rule_id)); CREATE TABLE model_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL); INSERT INTO model_meta VALUES ('schema_version','1'),('model','BAAI/bge-small-en-v1.5'),('revision','5c38ec7c405ec4b44b94cc5a9bb96e735b38267a'),('dimension','384'),('metric','cosine'),('normalized','1'),('encoding','little-endian-f32');").unwrap();
    c.execute("INSERT INTO tasks VALUES (1,'old task',?,NULL)", (task,))
        .unwrap();
    c.execute("INSERT INTO rules VALUES (1,'rule',?,?)", (rule, updated))
        .unwrap();
}
fn exec(p: &Path, args: &[&str]) -> (u8, String) {
    let _g = lock().lock().unwrap();
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(p).unwrap();
    let mut o = Vec::new();
    let mut e = Vec::new();
    let s = run(args.iter().map(|x| x.to_string()), &mut o, &mut e);
    std::env::set_current_dir(old).unwrap();
    assert!(e.is_empty(), "{e:?}");
    (s, String::from_utf8(o).unwrap())
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[test]
fn old_tasks_do_not_affect_fresh_rules_or_all_up_to_date() {
    let p = root("fresh");
    let n = now();
    store(&p, n - 30 * 86_400_000, n, None);
    let (s, out) = exec(&p, &["status", "--format", "json"]);
    assert_eq!(s, 0);
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["all_up_to_date"], true);
    assert!(v["stale_rules"].as_array().unwrap().is_empty());
    assert!(v.get("stale_tasks").is_none());
    fs::remove_dir_all(p).unwrap();
}
#[test]
fn stale_rule_is_reported_with_effective_timestamp_and_old_task_is_not() {
    let p = root("stale");
    let n = now();
    store(
        &p,
        n - 30 * 86_400_000,
        n - 30 * 86_400_000,
        Some(n - 30 * 86_400_000),
    );
    let (s, out) = exec(&p, &["status", "--format", "json"]);
    assert_eq!(s, 0);
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["all_up_to_date"], false);
    assert_eq!(v["stale_rules"][0]["id"], 1);
    assert_eq!(v["stale_rules"][0]["last_revised_at"], n - 30 * 86_400_000);
    assert!(v.get("stale_tasks").is_none());
    fs::remove_dir_all(p).unwrap();
}
#[test]
fn all_renderers_keep_window_and_rule_fields_without_task_fields() {
    let p = root("render");
    let n = now();
    store(
        &p,
        n - 30 * 86_400_000,
        n - 30 * 86_400_000,
        Some(n - 30 * 86_400_000),
    );
    for args in [
        vec!["status", "--format", "json"],
        vec!["status", "--format", "yaml"],
        vec!["status", "--format", "yaml", "--detailed"],
    ] {
        let (s, out) = exec(&p, &args);
        assert_eq!(s, 0);
        assert!(out.contains("window_days"));
        assert!(out.contains("stale_rules"));
        assert!(!out.contains("stale_tasks"));
        assert!(!out.contains("stale_tasks_count"));
    }
    fs::remove_dir_all(p).unwrap();
}
#[test]
fn null_updated_at_falls_back_to_created_at() {
    let p = root("fallback");
    let n = now();
    store(&p, n - 30 * 86_400_000, n - 30 * 86_400_000, None);
    let (s, out) = exec(&p, &["status", "--format", "json"]);
    assert_eq!(s, 0);
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["stale_rules"][0]["last_revised_at"], n - 30 * 86_400_000);
    fs::remove_dir_all(p).unwrap();
}
