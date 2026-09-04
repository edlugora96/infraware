//! Task 1.1 verification: the CLI parser accepts exactly `ingest`, `query`,
//! `list`, `get`, `patch`, `remove` and rejects every legacy verb, alias,
//! batch/legacy shape, and removed flag with status 2 and empty stdout.
//!
//! Run with `cargo test --test verbs`.

use infraware::run;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

/// Serializes CWD-mutating tests within this binary (see `contract.rs`).
static CWD_LOCK: Mutex<()> = Mutex::new(());

/// Run `run` against a string argv and capture stdout/stderr bytes and status.
fn execute(argv: &[&str]) -> (Vec<u8>, Vec<u8>, u8) {
    let args: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let mut stdout = Cursor::new(Vec::new());
    let mut stderr = Cursor::new(Vec::new());
    let code = run(args, &mut stdout, &mut stderr);
    (stdout.into_inner(), stderr.into_inner(), code)
}

/// RAII guard scoping the process working directory to a fresh temp dir.
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
            "infraware-verbs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp).expect("create temp working directory");
        std::env::set_current_dir(&temp).expect("enter temp working directory");
        TempCwd {
            _lock,
            original,
            _temp: temp,
        }
    }
}

impl Drop for TempCwd {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.original);
        let _ = std::fs::remove_dir_all(&self._temp);
    }
}

/// Every legacy verb, alias, and unknown command is a usage error (status 2)
/// with empty stdout and creates no storage.
#[test]
fn legacy_verbs_and_aliases_rejected_with_status_2_and_empty_stdout() {
    let _guard = TempCwd::new();
    let cases: &[&[&str]] = &[
        &["init"],
        &["analyze"],
        &["analyze", "--format", "json"],
        &["analyze", "--suggest"],
        &["insert", "abc", "--value", "x"],
        &["edit", "abc", "--value", "y"],
        &["set", "abc", "--value", "x"],
        &["rm", "abc"],
        &["foo"],
        &["status"],
        &["delete"],
    ];
    for argv in cases {
        let (stdout, stderr, status) = execute(argv);
        assert_eq!(
            status,
            infraware::EXIT_USAGE,
            "legacy/unknown {argv:?} must exit status 2"
        );
        assert!(stdout.is_empty(), "no stdout on usage rejection: {argv:?}");
        assert!(!stderr.is_empty(), "diagnostic on stderr: {argv:?}");
    }
    assert!(
        !std::env::current_dir()
            .expect("temp CWD is set by the guard")
            .join(".infraware")
            .exists(),
        "rejected verbs create no storage"
    );
}

/// Batch and legacy-flag shapes on the six verbs are usage errors (status 2)
/// with empty stdout: ingest takes at most one text plus one metadata arg,
/// and `--format`/`--suggest` no longer exist on any verb.
#[test]
fn batch_and_legacy_flag_shapes_rejected_with_status_2_and_empty_stdout() {
    let _guard = TempCwd::new();
    let cases: &[&[&str]] = &[
        &["ingest"],
        &["ingest", "a", "{}", "extra"],
        &["ingest", "hello", "--format"],
        &["query"],
        &["query", "a", "{}", "extra"],
        &["get"],
        &["get", "1", "extra"],
        &["get", "1", "--format", "json"],
        &["get", "1", "--suggest"],
        &["list", "--format", "json"],
        &["list", "--suggest"],
        &["list", "--verbose"],
        &["patch"],
        &["patch", "1"],
        &["patch", "1", "--merge", "x"],
        &["remove"],
        &["remove", "1", "extra"],
    ];
    for argv in cases {
        let (stdout, stderr, status) = execute(argv);
        assert_eq!(
            status,
            infraware::EXIT_USAGE,
            "rejected shape {argv:?} must exit status 2"
        );
        assert!(stdout.is_empty(), "no stdout on usage rejection: {argv:?}");
        assert!(!stderr.is_empty(), "diagnostic on stderr: {argv:?}");
    }
    assert!(
        !std::env::current_dir()
            .expect("temp CWD is set by the guard")
            .join(".infraware")
            .exists(),
        "rejected shapes create no storage"
    );
}

/// The six approved verbs remain parseable (accepted shapes are unchanged).
#[test]
fn six_approved_verb_shapes_still_parse() {
    assert!(infraware::parse_args(&["ingest".into(), "hello".into()]).is_ok());
    assert!(infraware::parse_args(&["query".into(), "hello".into()]).is_ok());
    assert!(infraware::parse_args(&["list".into()]).is_ok());
    assert!(infraware::parse_args(&["get".into(), "1".into()]).is_ok());
    assert!(
        infraware::parse_args(&["patch".into(), "1".into(), "--text".into(), "x".into()]).is_ok()
    );
    assert!(infraware::parse_args(&["remove".into(), "1".into()]).is_ok());
}
