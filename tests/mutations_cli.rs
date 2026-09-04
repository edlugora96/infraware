//! CLI-level coverage for semantic search `remove` and `patch` (tasks 6.2-6.4).
//!
//! Invokes the real `run` path (parse → dispatch → streams/status) under the
//! repository's `TempCwd` + `CWD_LOCK` discipline. Success paths that need a
//! live vector index are covered in-crate (`vec_store`, `remove`, `patch`,
//! `dispatch` unit tests) where the extension registers without FFI in the
//! test harness; this file covers what is inducible through the public CLI
//! without a model or vector extension:
//!
//! - usage rejections (status 2, empty stdout, no artifacts),
//! - missing-storage operationals (status 1, exactly one `error:` line,
//!   no leaked internals, no files created),
//! - record-not-found is unreachable here (it needs a seeded index) and is
//!   covered at the dispatch/unit layers instead.

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
            "infraware-mutations-{}-{}",
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

    fn assert_no_artifacts(&self) {
        assert!(
            !self._temp.join(".infraware").exists(),
            "rejected command must not create index/model artifacts"
        );
    }
}

impl Drop for TempCwd {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
        let _ = fs::remove_dir_all(&self._temp);
    }
}

fn assert_usage_rejected(cwd: &TempCwd, args: &[&str]) {
    let (status, stdout, stderr) = exec(args);
    assert_eq!(status, EXIT_USAGE, "args {args:?} must exit status 2");
    assert!(stdout.is_empty(), "args {args:?} must leave stdout empty");
    assert!(!stderr.is_empty(), "args {args:?} must explain on stderr");
    cwd.assert_no_artifacts();
}

fn assert_operational_rejected(cwd: &TempCwd, args: &[&str]) {
    let (status, stdout, stderr) = exec(args);
    assert_eq!(status, EXIT_FAILURE, "args {args:?} must exit status 1");
    assert!(stdout.is_empty(), "args {args:?} must leave stdout empty");
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "args {args:?} must emit exactly one stderr line, got: {stderr:?}"
    );
    assert!(
        lines[0].starts_with("error:"),
        "stderr must start with `error:`, got: {stderr:?}"
    );
    for leaked in [
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
        "768",
        "384",
    ] {
        assert!(
            !stderr.contains(leaked),
            "stderr must not leak {leaked:?}, got: {stderr:?}"
        );
    }
    // The temp dir path itself must never leak either.
    assert!(
        !stderr.contains(&cwd._temp.to_string_lossy().into_owned()),
        "stderr must not leak temp path, got: {stderr:?}"
    );
}

// ---- remove ----

#[test]
fn remove_missing_id_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["remove"]);
}

#[test]
fn remove_non_integer_id_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["remove", "abc"]);
}

#[test]
fn remove_zero_and_negative_ids_are_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["remove", "0"]);
    assert_usage_rejected(&cwd, &["remove", "-1"]);
}

#[test]
fn remove_extra_argument_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["remove", "1", "extra"]);
}

#[test]
fn remove_on_missing_storage_is_operational() {
    let cwd = TempCwd::new();
    assert_operational_rejected(&cwd, &["remove", "1"]);
    cwd.assert_no_artifacts();
}

// ---- patch ----

#[test]
fn patch_missing_id_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["patch"]);
}

#[test]
fn patch_absent_payload_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["patch", "1"]);
}

#[test]
fn patch_non_integer_id_is_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["patch", "abc", "--text", "x"]);
    assert_usage_rejected(&cwd, &["patch", "0", "--text", "x"]);
    assert_usage_rejected(&cwd, &["patch", "-2", "--metadata", "{}"]);
}

#[test]
fn patch_unknown_and_duplicate_flags_are_usage() {
    let cwd = TempCwd::new();
    assert_usage_rejected(&cwd, &["patch", "1", "--merge", "x"]);
    assert_usage_rejected(&cwd, &["patch", "1", "--text", "a", "--text", "b"]);
    assert_usage_rejected(
        &cwd,
        &["patch", "1", "--metadata", "{}", "--metadata", "{}"],
    );
    assert_usage_rejected(&cwd, &["patch", "1", "--text"]);
    assert_usage_rejected(&cwd, &["patch", "1", "--metadata"]);
    assert_usage_rejected(&cwd, &["patch", "1", "extra", "--text", "x"]);
}

#[test]
fn patch_invalid_payload_is_usage() {
    let cwd = TempCwd::new();
    // Whitespace-only text, malformed metadata, and non-flat metadata are
    // usage errors raised before any storage work.
    assert_usage_rejected(&cwd, &["patch", "1", "--text", "   "]);
    assert_usage_rejected(&cwd, &["patch", "1", "--metadata", "{bad}"]);
    assert_usage_rejected(&cwd, &["patch", "1", "--metadata", r#"{"a":1}"#]);
}

#[test]
fn patch_on_missing_storage_is_operational() {
    let cwd = TempCwd::new();
    assert_operational_rejected(&cwd, &["patch", "1", "--text", "hello"]);
    assert_operational_rejected(&cwd, &["patch", "1", "--metadata", r#"{"a":"1"}"#]);
    cwd.assert_no_artifacts();
}
