//! Acceptance tests for tasks 2.1-2.4: read-only list/get verbs.
//!
//! Drives the public `run` entry (parse -> dispatch -> streams/status)
//! under a TempCwd guard. Seeds via `ingest` (lazy SQLite create), then
//! verifies flat filter/paging/snapshot total, ascending ids, exact four
//! record keys with full text bytes, direct get existing/missing, and
//! read-only missing-store behavior.

use infraware::{EXIT_FAILURE, run};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

static CWD_LOCK: Mutex<()> = Mutex::new(());

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

fn exec(args: &[&str]) -> (u8, String, String) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let status = run(argv(args), &mut stdout, &mut stderr);
    (
        status,
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

struct TempCwd {
    _lock: MutexGuard<'static, ()>,
    original: PathBuf,
    temp: PathBuf,
}

impl TempCwd {
    fn new() -> TempCwd {
        let _lock = CWD_LOCK.lock().expect("CWD lock poisoned");
        let original = std::env::current_dir().expect("resolve original cwd");
        let temp = std::env::temp_dir().join(format!(
            "infraware-readpaths-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp).expect("create temp dir");
        std::env::set_current_dir(&temp).expect("enter temp dir");
        TempCwd {
            _lock,
            original,
            temp,
        }
    }
}

impl Drop for TempCwd {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
        let _ = std::fs::remove_dir_all(&self.temp);
    }
}

fn seed_three(root: &TempCwd) -> (i64, i64, i64) {
    let _ = root;
    for (text, meta) in [
        ("alpha one", r#"{"tag":"a"}"#),
        ("beta two", r#"{"tag":"b"}"#),
        ("  alpha two spaced  ", r#"{"tag":"a"}"#),
    ] {
        let (st, _, _) = exec(&["ingest", text, meta]);
        assert_eq!(st, 0, "seed ingest must succeed");
    }
    // Discover ids via unfiltered list (ascending order).
    let (st, out, _) = exec(&["list"]);
    assert_eq!(st, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let ids: Vec<i64> = v["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids.len(), 3);
    (ids[0], ids[1], ids[2])
}

#[test]
fn list_filter_paging_snapshot_total_and_order() {
    let root = TempCwd::new();
    let (id1, _id2, id3) = seed_three(&root);
    // 2.1 flat filter: only tag=a rows, ascending ids, total == full filtered count.
    let (st, out, _) = exec(&["list", r#"{"tag":"a"}"#]);
    assert_eq!(st, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["total"], 2);
    let recs = v["records"].as_array().unwrap();
    assert_eq!(recs.len(), 2);
    assert_eq!(recs[0]["id"], id1);
    assert_eq!(recs[1]["id"], id3);
    // 2.2 paging: limit 1 offset 1 over filtered set -> second row, total still 2.
    let (st, out, _) = exec(&["list", r#"{"tag":"a"}"#, "1", "1"]);
    assert_eq!(st, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        v["total"], 2,
        "total is snapshot-consistent, not page length"
    );
    assert_eq!(v["records"].as_array().unwrap().len(), 1);
    assert_eq!(v["records"][0]["id"], id3);
    // Offset past end: empty page, total preserved.
    let (st, out, _) = exec(&["list", r#"{"tag":"a"}"#, "50", "99"]);
    assert_eq!(st, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["total"], 2);
    assert_eq!(v["records"].as_array().unwrap().len(), 0);
}

#[test]
fn list_records_carry_exact_keys_with_full_text() {
    let root = TempCwd::new();
    seed_three(&root);
    let (st, out, _) = exec(&["list"]);
    assert_eq!(st, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["total"], 3);
    for r in v["records"].as_array().unwrap() {
        let obj = r.as_object().unwrap();
        assert_eq!(obj.len(), 4, "exactly id/text/metadata/created_at");
        assert!(
            obj.contains_key("id")
                && obj.contains_key("text")
                && obj.contains_key("metadata")
                && obj.contains_key("created_at")
        );
    }
    // Full text bytes preserved (leading/trailing spaces).
    let texts: Vec<&str> = v["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["text"].as_str().unwrap())
        .collect();
    assert!(texts.contains(&"  alpha two spaced  "));
}

#[test]
fn get_existing_and_missing() {
    let root = TempCwd::new();
    let (id1, _, _) = seed_three(&root);
    let (st, out, _) = exec(&["get", &id1.to_string()]);
    assert_eq!(st, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["id"], id1);
    assert_eq!(v["text"], serde_json::json!("alpha one"));
    assert_eq!(v["metadata"], serde_json::json!({"tag": "a"}));
    assert_eq!(v.as_object().unwrap().len(), 4);
    // Missing id: typed record-not-found -> status 1, empty stdout, one error line.
    let (st, out, err) = exec(&["get", "999999"]);
    assert_eq!(st, EXIT_FAILURE);
    assert!(out.is_empty());
    assert_eq!(err.lines().count(), 1);
    assert!(err.starts_with("error:"));
}

#[test]
fn read_verbs_on_missing_store_fail_closed_without_creating() {
    let _root = TempCwd::new();
    for args in [
        vec!["list"],
        vec!["get", "1"],
        vec!["list", r#"{"tag":"a"}"#, "10", "0"],
    ] {
        let (st, out, err) = exec(&args);
        assert_eq!(st, EXIT_FAILURE, "{args:?} must be operational");
        assert!(out.is_empty(), "{args:?}: empty stdout");
        assert_eq!(err.lines().count(), 1, "{args:?}: one error line");
        assert!(err.starts_with("error:"));
    }
    assert!(
        !std::env::current_dir().unwrap().join(".infraware").exists(),
        "reads must not create storage"
    );
}
