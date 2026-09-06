//! v2 list/get output and read-only behavior.
use infraware::{EXIT_FAILURE, run};
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

fn cwd_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
fn root() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let p = std::env::temp_dir().join(format!("infraware-read-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&p).unwrap();
    p
}
fn exec(p: &Path, a: &[&str]) -> (u8, String, String) {
    let _lock = cwd_lock().lock().unwrap();
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(p).unwrap();
    let mut o = Vec::new();
    let mut e = Vec::new();
    let args: Vec<String> = a.iter().map(|x| x.to_string()).collect();
    let s = run(args, &mut o, &mut e);
    std::env::set_current_dir(old).unwrap();
    (
        s,
        String::from_utf8(o).unwrap(),
        String::from_utf8(e).unwrap(),
    )
}
fn compatible_store(p: &Path) {
    let dir = p.join(".infraware");
    fs::create_dir_all(&dir).unwrap();
    let db = dir.join("vec-index.sqlite");
    let conn = Connection::open(db).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=DELETE;
         CREATE TABLE tasks(task_id INTEGER PRIMARY KEY,text TEXT NOT NULL,created_at INTEGER NOT NULL,updated_at INTEGER NULL);
         CREATE TABLE rules(rule_id INTEGER PRIMARY KEY,text TEXT NOT NULL,created_at INTEGER NOT NULL,updated_at INTEGER NULL);
         CREATE TABLE task_vecs(rowid INTEGER PRIMARY KEY,embedding BLOB NOT NULL,CHECK(embedding IS NOT 'float[384] cosine'));
         CREATE TABLE rule_vecs(rowid INTEGER PRIMARY KEY,embedding BLOB NOT NULL,CHECK(embedding IS NOT 'float[384] cosine'));
         CREATE TABLE task_rule_relations(task_id INTEGER NOT NULL,rule_id INTEGER NOT NULL,created_at INTEGER,PRIMARY KEY(task_id,rule_id));
         CREATE TABLE model_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
         INSERT INTO model_meta VALUES
           ('schema_version','1'),('model','BAAI/bge-small-en-v1.5'),
           ('revision','5c38ec7c405ec4b44b94cc5a9bb96e735b38267a'),
           ('dimension','384'),('metric','cosine'),('normalized','1'),
           ('encoding','little-endian-f32');
         INSERT INTO rules VALUES (1,'first rule',10,NULL),(2,'second rule',20,22);
         INSERT INTO tasks VALUES (7,'early task',5,NULL),(8,'late task',30,31);
         INSERT INTO task_rule_relations VALUES (7,2,1),(8,2,2);",
    )
    .unwrap();
}

fn assert_one_stdout_newline(stdout: &str) {
    assert!(stdout.ends_with('\n'));
    assert!(!stdout.ends_with("\n\n"));
}

#[test]
fn list_end_to_end_formats_grouped_fields_and_streams() {
    let p = root();
    compatible_store(&p);

    let (status, yaml, stderr) = exec(&p, &["list"]);
    assert_eq!(status, 0);
    assert!(stderr.is_empty());
    assert_one_stdout_newline(&yaml);
    let value: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(value["limit"], 50);
    assert_eq!(value["offset"], 0);
    assert_eq!(value["total"], 2);
    assert_eq!(value["rules"][0]["text"], "first rule");
    assert_eq!(value["rules"][0]["id"], 1);
    assert_eq!(
        value["rules"][0]["tasks"],
        serde_yaml::Value::Sequence(vec![])
    );
    assert_eq!(value["rules"][1]["tasks"][0]["id"], 7);
    assert_eq!(value["rules"][1]["tasks"][0]["text"], "early task");
    assert_eq!(value["rules"][0].as_mapping().unwrap().len(), 3);
    assert_eq!(value["rules"][1]["tasks"][0].as_mapping().unwrap().len(), 2);
    assert!(yaml.contains("rules:"));
    assert!(!yaml.contains("records:"));
    assert!(!yaml.contains("entity:"));
    assert!(!yaml.contains("type:"));

    let (status, json, stderr) = exec(&p, &["list", "--format", "json"]);
    assert_eq!(status, 0);
    assert!(stderr.is_empty());
    assert_one_stdout_newline(&json);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["total"], 2);
    assert_eq!(value["rules"][1]["tasks"][1]["text"], "late task");
    assert_eq!(value.as_object().unwrap().len(), 4);
    assert_eq!(value["rules"][0].as_object().unwrap().len(), 3);
    assert_eq!(value["rules"][1]["tasks"][0].as_object().unwrap().len(), 2);
    assert!(value["rules"][0].get("created_at").is_none());
    assert!(value["rules"][1]["tasks"][0].get("updated_at").is_none());
    assert!(json.find("records").is_none());

    for format in ["yaml", "json"] {
        let args = ["list", "--format", format, "--detailed"];
        let (status, output, stderr) = exec(&p, &args);
        assert_eq!(status, 0);
        assert!(stderr.is_empty());
        assert_one_stdout_newline(&output);
        if format == "yaml" {
            let value: serde_yaml::Value = serde_yaml::from_str(&output).unwrap();
            assert_eq!(value["rules"][0]["created_at"], "56 years ago");
            assert!(value["rules"][0].get("updated_at").is_none());
            assert_eq!(value["rules"][1]["updated_at"], 22);
            assert_eq!(value["rules"][1]["tasks"][0]["created_at"], "56 years ago");
            assert!(value["rules"][1]["tasks"][0].get("updated_at").is_none());
            assert_eq!(value["rules"][1]["tasks"][1]["updated_at"], 31);
        } else {
            let value: serde_json::Value = serde_json::from_str(&output).unwrap();
            assert_eq!(value["rules"][0]["created_at"], 10);
            assert!(value["rules"][0].get("updated_at").is_none());
            assert_eq!(value["rules"][1]["tasks"][0]["created_at"], 5);
            assert!(value["rules"][1]["tasks"][0].get("updated_at").is_none());
            assert_eq!(value["rules"][1]["tasks"][1]["updated_at"], 31);
        }
    }
    let _ = fs::remove_dir_all(p);
}

#[test]
fn list_missing_and_incompatible_stores_fail_read_only() {
    let missing = root();
    let (status, stdout, stderr) = exec(&missing, &["list"]);
    assert_eq!(status, EXIT_FAILURE);
    assert!(stdout.is_empty());
    assert_eq!(stderr.lines().count(), 1);
    assert!(stderr.starts_with("error:"));
    assert!(!missing.join(".infraware").exists());
    let _ = fs::remove_dir_all(missing);

    let incompatible = root();
    let db = incompatible.join(".infraware/vec-index.sqlite");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    Connection::open(&db)
        .unwrap()
        .execute_batch("CREATE TABLE legacy(id INTEGER); INSERT INTO legacy VALUES (7);")
        .unwrap();
    let before = fs::read(&db).unwrap();
    let (status, stdout, stderr) = exec(&incompatible, &["list"]);
    assert_eq!(status, EXIT_FAILURE);
    assert!(stdout.is_empty());
    assert_eq!(stderr.lines().count(), 1);
    assert!(stderr.starts_with("error:"));
    assert!(!stderr.contains("infraware ingest"));
    assert_eq!(fs::read(&db).unwrap(), before);
    let _ = fs::remove_dir_all(incompatible);
}

#[test]
fn corrupt_store_stays_generic_and_unchanged() {
    let p = root();
    let db = p.join(".infraware/vec-index.sqlite");
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    fs::write(&db, b"not sqlite").unwrap();
    let before = fs::read(&db).unwrap();
    let (status, stdout, stderr) = exec(&p, &["status"]);
    assert_eq!(status, EXIT_FAILURE);
    assert!(stdout.is_empty());
    assert_eq!(stderr, "error: i/o error: store unavailable\n");
    assert!(!stderr.contains("infraware ingest"));
    assert_eq!(fs::read(&db).unwrap(), before);
    let _ = fs::remove_dir_all(p);
}

#[test]
fn all_non_ingest_verbs_report_typed_missing_storage() {
    let p = root();
    let hint = "error: canonical storage is not initialized; run `infraware ingest <TASK_TEXT> <RULE_TEXT>` first\n";
    for a in [
        &["query", "task"][..],
        &["list"][..],
        &["get", "1"][..],
        &["patch", "1", "--text", "changed"][..],
        &["remove", "1", "--entity", "tasks"][..],
        &["status"][..],
        &["revise", "1", "--entity", "rules"][..],
    ] {
        let (s, o, e) = exec(&p, a);
        assert_eq!(s, EXIT_FAILURE, "{a:?}");
        assert!(o.is_empty(), "{a:?}");
        assert_eq!(e, hint, "{a:?}");
    }
    assert!(!p.join(".infraware").exists());
    let _ = std::fs::remove_dir_all(p);
}
#[test]
fn reads_missing_store_are_read_only() {
    let p = root();
    let (s, o, e) = exec(&p, &["list"]);
    assert_eq!(s, EXIT_FAILURE);
    assert!(o.is_empty());
    assert_eq!(e.lines().count(), 1);
    assert!(e.starts_with("error:"));
    assert!(!p.join(".infraware").exists());
    let _ = std::fs::remove_dir_all(p);
}
