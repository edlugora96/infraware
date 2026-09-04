//! Task 7.1: serialized CLI-level usage validation for `query`.
//!
//! Invokes the real `run` path (parse → dispatch → streams/status) under the
//! repository's `TempCwd` + `CWD_LOCK` discipline. Every case asserts the
//! frozen usage contract: status 2 (`EXIT_USAGE`), empty stdout, non-empty
//! stderr, and no index/model work attempted (no `.infraware` artifact
//! created in the temp root).

use infraware::{EXIT_FAILURE, EXIT_USAGE, run};
use std::fs;
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
    _temp: PathBuf,
}

impl TempCwd {
    fn new() -> TempCwd {
        let _lock = CWD_LOCK.lock().expect("CWD lock poisoned");
        let original = std::env::current_dir().expect("resolve original working directory");
        let temp = std::env::temp_dir().join(format!(
            "infraware-query-usage-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp).expect("create temp working directory");
        std::env::set_current_dir(&temp).expect("enter temp working directory");
        TempCwd {
            _lock,
            original,
            _temp: temp,
        }
    }

    fn assert_no_index_or_model_work(&self) {
        assert!(
            !self._temp.join(".infraware").exists(),
            "usage rejection must not create index/model artifacts"
        );
    }
}

impl Drop for TempCwd {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
        let _ = fs::remove_dir_all(&self._temp);
    }
}

/// Shared assertion: status 2, empty stdout, diagnostics on stderr, and no
/// index/model side effects.
fn assert_usage_rejected(cwd: &TempCwd, args: &[&str]) {
    let (status, stdout, stderr) = exec(args);
    assert_eq!(status, EXIT_USAGE, "args {args:?} must exit status 2");
    assert!(stdout.is_empty(), "args {args:?} must leave stdout empty");
    assert!(!stderr.is_empty(), "args {args:?} must explain on stderr");
    cwd.assert_no_index_or_model_work();
}

#[test]
fn query_missing_text_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["query"]);
}

#[test]
fn query_blank_text_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["query", "   "]);
}

#[test]
fn query_empty_text_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["query", ""]);
}

#[test]
fn query_extra_positional_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["query", "a", "{}", "extra"]);
}

#[test]
fn query_flag_as_second_positional_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["query", "a", "--json"]);
}

#[test]
fn query_flag_as_first_positional_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["query", "--help"]);
}

#[test]
fn query_malformed_filter_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["query", "hello", "{bad}"]);
}

#[test]
fn query_array_filter_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["query", "hello", r#"["a"]"#]);
}

#[test]
fn query_nested_filter_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["query", "hello", r#"{"a":{"b":"c"}}"#]);
}

#[test]
fn query_non_string_filter_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["query", "hello", r#"{"a":1}"#]);
}

#[test]
fn query_duplicate_key_filter_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["query", "hello", r#"{"a":"1","a":"2"}"#]);
}

#[test]
fn query_bad_key_filter_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["query", "hello", r#"{"a b":"1"}"#]);
}

#[test]
fn query_unquoted_filter_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["query", "hello", r#"{a:"1"}"#]);
}
// Task 7.2 (appended): serialized CLI-level operational-failure coverage for
// `query` (status-1 storage / model / embed / vector cases inducible offline).
//
// Every case exercises the real CLI path under `TempCwd` + `CWD_LOCK` and
// asserts the frozen operational contract: status 1 (`EXIT_FAILURE`), empty
// stdout, exactly one `error:` stderr line, and no leaked internals (paths,
// SQL, vectors, model/cache/revision/dimension/network details).
//
// Storage cases run the in-process `run` adapter (no environment mutation).
// Model-cache cases run the compiled binary as a subprocess with
// `current_dir` pointed at the guarded temp dir and a controlled
// `XDG_CACHE_HOME`, so no process-global environment mutation (and no
// `unsafe`) is needed. Embed/`vec` runtime failures past the setup gate
// require the full pinned model cache (133 MB weights via network) and are
// NOT inducible offline here; they are flagged in the task report rather
// than weakened.

const MODEL_REVISION_72: &str = "5c38ec7c405ec4b44b94cc5a9bb96e735b38267a";

/// Shared assertion over captured (status, stdout, stderr) for operational
/// rejection. `temp` is the temp-dir path that must never leak into stderr.
fn assert_operational_output(temp: &std::path::Path, status: u8, stdout: &str, stderr: &str) {
    assert_eq!(status, EXIT_FAILURE, "must exit status 1");
    assert!(stdout.is_empty(), "must leave stdout empty");
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "must emit exactly one stderr line, got: {stderr:?}"
    );
    assert!(
        lines[0].starts_with("error:"),
        "stderr line must start with `error:`, got: {stderr:?}"
    );
    let temp = temp.to_string_lossy().into_owned();
    for leaked in [
        temp.as_str(),
        ".infraware",
        ".sqlite",
        "vec-index",
        "vecs",
        "SELECT",
        "select",
        "embedding",
        "model",
        "cache",
        "revision",
        "dimension",
        "metric",
        "cosine",
        "network",
        "download",
        "http",
        "token",
        "768",
        "384",
    ] {
        assert!(
            !stderr.contains(leaked),
            "stderr must not leak {leaked:?}, got: {stderr:?}"
        );
    }
}

/// Shared assertion for in-process `run`: status 1, empty stdout, exactly
/// one `error:` stderr line, no leaked internals.
fn assert_operational_rejected(cwd: &TempCwd, args: &[&str]) {
    let (status, stdout, stderr) = exec(args);
    assert_operational_output(&cwd._temp, status, &stdout, &stderr);
}

/// Seed a query-compatible `vec-index.sqlite` under the temp CWD using only
/// plain SQLite (no vector extension needed): the query compatibility gate
/// only reads `sqlite_master` for the `vecs` SQL marker plus the side
/// tables, index, and six pinned `model_meta` values, and every 7.2 case
/// fails before any vector scan runs. Returns the DB path.
fn seed_valid_index_72(cwd: &TempCwd) -> PathBuf {
    let dir = cwd._temp.join(".infraware");
    fs::create_dir_all(&dir).expect("create .infraware dir");
    let db = dir.join("vec-index.sqlite");
    let conn = rusqlite::Connection::open(&db).expect("open seed db");
    // The `vecs` table carries the exact marker substrings the
    // compatibility gate requires (`float[384]`, `cosine`) inside a
    // no-op CHECK, so no `vec0` extension is needed: no scan ever runs
    // on the 7.2 failure paths (verify/model fail first).
    conn.execute_batch(
        "PRAGMA journal_mode=DELETE;
         CREATE TABLE records(
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           hash TEXT NOT NULL UNIQUE,
           text TEXT NOT NULL,
           metadata TEXT NOT NULL,
           created_ms INTEGER NOT NULL DEFAULT (strftime('%s','now')*1000)
         );
         CREATE TABLE meta_kv(
           record_id INTEGER NOT NULL REFERENCES records(id),
           k TEXT NOT NULL,
           v TEXT NOT NULL,
           PRIMARY KEY(record_id,k)
         );
         CREATE INDEX idx_meta_kv_kv ON meta_kv(k,v);
         CREATE TABLE vecs(
           rowid INTEGER PRIMARY KEY,
           embedding BLOB NOT NULL,
           CHECK(embedding IS NOT 'float[384] cosine')
         );
         CREATE TABLE model_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);",
    )
    .expect("seed schema");
    for (k, v) in [
        ("schema_version", "1"),
        ("model", "BAAI/bge-small-en-v1.5"),
        ("revision", MODEL_REVISION_72),
        ("dimension", "384"),
        ("metric", "cosine"),
        ("normalized", "1"),
    ] {
        conn.execute(
            "INSERT INTO model_meta(key,value) VALUES(?1,?2)",
            rusqlite::params![k, v],
        )
        .expect("seed model_meta");
    }
    drop(conn);
    db
}

/// Run the compiled binary as a subprocess with `current_dir` = the guarded
/// temp dir and `XDG_CACHE_HOME` pointed at `cache_home` (which the caller
/// leaves missing or poisons). No process-global mutation is needed.
fn exec_binary_in(
    temp: &std::path::Path,
    cache_home: &std::path::Path,
    args: &[&str],
) -> (Option<i32>, String, String) {
    let bin = env!("CARGO_BIN_EXE_infraware");
    let out = std::process::Command::new(bin)
        .current_dir(temp)
        .env("XDG_CACHE_HOME", cache_home)
        .args(args)
        .output()
        .expect("spawn query binary");
    (
        out.status.code(),
        String::from_utf8(out.stdout).expect("stdout utf8"),
        String::from_utf8(out.stderr).expect("stderr utf8"),
    )
}

#[test]
fn query_missing_index_is_operational() {
    let cwd = TempCwd::new();
    assert_operational_rejected(&cwd, &["query", "hello"]);
    assert!(
        !cwd._temp.join(".infraware").exists(),
        "missing index must not create index/model artifacts"
    );
}

#[test]
fn query_unreadable_index_is_operational() {
    let cwd = TempCwd::new();
    // A directory at the DB path is not a readable index file.
    let dir = cwd._temp.join(".infraware");
    fs::create_dir_all(dir.join("vec-index.sqlite")).expect("plant unreadable index");
    assert_operational_rejected(&cwd, &["query", "hello"]);
}

#[test]
fn query_incompatible_schema_is_operational() {
    let cwd = TempCwd::new();
    // A valid SQLite file with none of the expected tables.
    let dir = cwd._temp.join(".infraware");
    fs::create_dir_all(&dir).expect("create .infraware dir");
    let conn = rusqlite::Connection::open(dir.join("vec-index.sqlite")).expect("open dummy db");
    conn.execute_batch("CREATE TABLE dummy(id INTEGER PRIMARY KEY);")
        .expect("dummy schema");
    drop(conn);
    assert_operational_rejected(&cwd, &["query", "hello"]);
}

#[test]
fn query_incompatible_model_meta_is_operational() {
    let cwd = TempCwd::new();
    let db = seed_valid_index_72(&cwd);
    let conn = rusqlite::Connection::open(&db).expect("open seeded db");
    conn.execute(
        "UPDATE model_meta SET value='768' WHERE key='dimension'",
        [],
    )
    .expect("tamper dimension");
    drop(conn);
    assert_operational_rejected(&cwd, &["query", "hello"]);
}

#[test]
fn query_missing_cached_model_is_operational() {
    let cwd = TempCwd::new();
    seed_valid_index_72(&cwd);
    // Point the cache at a fresh empty dir: the cached-only accessor finds
    // nothing (no network, no download) and the CLI must fail operationally.
    let cache_home = cwd._temp.join("empty-cache-home");
    fs::create_dir_all(&cache_home).expect("create empty cache home");
    let (code, stdout, stderr) = exec_binary_in(&cwd._temp, &cache_home, &["query", "hello"]);
    assert_eq!(code, Some(EXIT_FAILURE as i32), "must exit status 1");
    assert_operational_output(&cwd._temp, EXIT_FAILURE, &stdout, &stderr);
}

#[test]
fn query_corrupt_cached_model_is_operational() {
    let cwd = TempCwd::new();
    seed_valid_index_72(&cwd);
    // A revision dir with only a corrupt manifest: validation fails with no
    // network/download, and the CLI must fail operationally.
    let cache_home = cwd._temp.join("corrupt-cache-home");
    let rev = cache_home
        .join("infraware/models/bge-small-en-v1.5")
        .join(MODEL_REVISION_72);
    fs::create_dir_all(&rev).expect("create corrupt revision dir");
    fs::write(rev.join("manifest.json"), b"{corrupt!!!").expect("poison manifest");
    let (code, stdout, stderr) = exec_binary_in(&cwd._temp, &cache_home, &["query", "hello"]);
    assert_eq!(code, Some(EXIT_FAILURE as i32), "must exit status 1");
    assert_operational_output(&cwd._temp, EXIT_FAILURE, &stdout, &stderr);
}
