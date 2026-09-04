//! Contract (integration) tests for the thin binary adapter: stream separation,
//! exit-status mapping, and caller-independent request construction.

use infraware::{Command, run};
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

/// Serializes every test that observes or mutates the process working directory.
///
/// `run` resolves the repository root via `std::env::current_dir()`, so
/// CWD-mutating `TempCwd` guards must not run concurrently with any test that
/// reads the CWD (a parallel test would otherwise observe another test's temp
/// dir). Holding this lock for the lifetime of each `TempCwd` (and for the
/// duration of CWD-reading `run`/`dispatch` tests) serializes the CWD access.
static CWD_LOCK: Mutex<()> = Mutex::new(());

/// Run `run` against a string argv and capture stdout/stderr bytes and status.
fn execute(argv: &[&str]) -> (Vec<u8>, Vec<u8>, u8) {
    let args: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let mut stdout = Cursor::new(Vec::new());
    let mut stderr = Cursor::new(Vec::new());
    let code = run(args, &mut stdout, &mut stderr);
    (stdout.into_inner(), stderr.into_inner(), code)
}

/// RAII guard that scopes the process working directory to a fresh, unique temp
/// directory for the lifetime of the guard, restoring the original working
/// directory on drop (including on panic).
///
/// `run` resolves the repository root via `std::env::current_dir()`, so tests
/// that seed a SQLite index (`ingest`) must not pollute the repository and
/// execute under this guard. Rejection tests perform no storage writes.
struct TempCwd {
    _lock: MutexGuard<'static, ()>,
    original: PathBuf,
    _temp: PathBuf,
}

impl TempCwd {
    /// Acquire the CWD lock, create a unique temp dir and make it the current directory.
    fn new() -> TempCwd {
        let _lock = CWD_LOCK.lock().expect("CWD lock poisoned");
        let original = std::env::current_dir().expect("resolve original working directory");
        let temp = std::env::temp_dir().join(format!(
            "infraware-contract-{}-{}",
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
        // Restore the original working directory so other tests/process state
        // are unaffected, then clean up the temp dir.
        let _ = std::env::set_current_dir(&self.original);
        let _ = std::fs::remove_dir_all(&self._temp);
    }
}

#[test]
fn init_rejected_as_usage_with_empty_stdout() {
    // The legacy `init` verb no longer exists: usage status 2 with empty
    // stdout, and no storage artifact is created in the working directory.
    let _guard = TempCwd::new();
    let (stdout, stderr, status) = execute(&["init"]);
    assert_eq!(status, infraware::EXIT_USAGE);
    assert!(stdout.is_empty(), "no stdout on usage rejection");
    assert!(!stderr.is_empty());
    assert!(
        !std::env::current_dir()
            .expect("temp CWD is set by the guard")
            .join(".infraware")
            .exists(),
        "rejected init creates no storage"
    );
}

#[test]
fn analyze_rejected_as_usage_with_empty_stdout() {
    // The legacy `analyze` verb no longer exists: usage status 2 with empty
    // stdout, diagnostics on stderr only.
    let _lock = CWD_LOCK.lock().expect("CWD lock poisoned");
    for argv in [
        ["analyze"].as_slice(),
        ["analyze", "--format", "json"].as_slice(),
        ["analyze", "--suggest"].as_slice(),
    ] {
        let (stdout, stderr, status) = execute(argv);
        assert_eq!(status, infraware::EXIT_USAGE);
        assert!(stdout.is_empty(), "no stdout on usage rejection");
        assert!(!stderr.is_empty());
    }
}

#[test]
fn get_missing_store_is_nonzero_and_diagnostic_on_stderr_only() {
    // Isolate from any index so the read fails closed: operational status
    // with the diagnostic on stderr and empty stdout, creating nothing.
    let _guard = TempCwd::new();
    let (stdout, stderr, status) = execute(&["get", "1"]);
    assert_ne!(status, 0);
    assert!(!stderr.is_empty(), "diagnostic must be on stderr");
    assert!(stdout.is_empty(), "no structured output on failed get");
}

#[test]
fn get_non_integer_id_is_usage_status() {
    // Record ids are positive integers: a non-integer id is a usage error
    // (status 2) with empty stdout, before any storage work.
    let _guard = TempCwd::new();
    let (stdout, stderr, status) = execute(&["get", "abc"]);
    assert_eq!(status, infraware::EXIT_USAGE);
    assert!(!stderr.is_empty());
    assert!(stdout.is_empty());
}

#[test]
fn insert_rejected_as_usage_with_empty_stdout() {
    // The legacy `insert` verb no longer exists: usage status 2 with empty
    // stdout, regardless of storage state.
    let _guard = TempCwd::new();
    let (stdout, stderr, status) = execute(&["insert", "abc", "--value", "x"]);
    assert_eq!(status, infraware::EXIT_USAGE);
    assert!(!stderr.is_empty());
    assert!(stdout.is_empty());
}

#[test]
fn edit_rejected_as_usage_with_empty_stdout() {
    // The legacy `edit` verb no longer exists: usage status 2 with empty
    // stdout, regardless of storage state.
    let _guard = TempCwd::new();
    let (stdout, stderr, status) = execute(&["edit", "abc", "--value", "y"]);
    assert_eq!(status, infraware::EXIT_USAGE);
    assert!(!stderr.is_empty());
    assert!(stdout.is_empty());
}

#[test]
fn remove_requires_storage_nonzero_stderr_only() {
    let _guard = TempCwd::new();
    let (stdout, stderr, status) = execute(&["remove", "abc"]);
    assert_ne!(status, 0);
    assert!(!stderr.is_empty());
    assert!(stdout.is_empty());
}

#[test]
fn unknown_command_is_usage_status_nonzero_stderr_only() {
    let (stdout, stderr, status) = execute(&["foo"]);
    // Distinct usage status (2) and no stdout.
    assert_eq!(status, infraware::EXIT_USAGE);
    assert_ne!(status, 0);
    assert!(!stderr.is_empty());
    assert!(stdout.is_empty());
}

#[test]
fn set_alias_rejected_as_usage() {
    let (stdout, stderr, status) = execute(&["set", "abc", "--value", "x"]);
    assert_eq!(status, infraware::EXIT_USAGE);
    assert!(!stderr.is_empty());
    assert!(stdout.is_empty());
}

#[test]
fn rm_alias_rejected_as_usage() {
    let (stdout, stderr, status) = execute(&["rm", "abc"]);
    assert_eq!(status, infraware::EXIT_USAGE);
    assert!(!stderr.is_empty());
    assert!(stdout.is_empty());
}

#[test]
fn legacy_format_flag_no_longer_parses() {
    // `--format` served only the Markdown path: it is now an unknown option
    // (usage status 2) on every verb that once accepted it, with empty stdout.
    for argv in [
        ["get", "1", "--format", "json"].as_slice(),
        ["list", "--format", "json"].as_slice(),
    ] {
        let (stdout, stderr, status) = execute(argv);
        assert_eq!(status, infraware::EXIT_USAGE);
        assert!(!stderr.is_empty());
        assert!(stdout.is_empty());
    }
}

#[test]
fn malformed_get_no_id_is_usage_status() {
    let (stdout, stderr, status) = execute(&["get"]);
    assert_eq!(status, infraware::EXIT_USAGE);
    assert!(!stderr.is_empty());
    assert!(stdout.is_empty());
}

#[test]
fn unknown_option_is_usage_status() {
    let (stdout, stderr, status) = execute(&["list", "--force"]);
    assert_eq!(status, infraware::EXIT_USAGE);
    assert!(!stderr.is_empty());
    assert!(stdout.is_empty());
}

#[test]
fn legacy_format_shape_is_rejected_at_parse_level() {
    // Accepted shapes do not error; `--format` is no longer an accepted
    // shape on any verb, so the parser rejects it as a usage error.
    let args: Vec<String> = vec!["get".into(), "1".into(), "--format".into(), "json".into()];
    assert!(infraware::parse_args(&args).unwrap_err().is_usage());
    let args: Vec<String> = vec!["list".into(), "--format".into(), "json".into()];
    assert!(infraware::parse_args(&args).unwrap_err().is_usage());
    // The bare six-verb shapes still parse.
    let args: Vec<String> = vec!["get".into(), "1".into()];
    assert_eq!(
        infraware::parse_args(&args).expect("accepted shape"),
        Command::Get {
            id: "1".to_string(),
        }
    );
    let args: Vec<String> = vec!["list".into()];
    assert_eq!(
        infraware::parse_args(&args).expect("accepted shape"),
        Command::List {
            filter: Default::default(),
            limit: 50,
            offset: 0,
        }
    );
}

#[test]
fn caller_independent_request_construction_is_equal() {
    // Serialized: dispatch with relative root `.` resolves against the CWD.
    let _lock = CWD_LOCK.lock().expect("CWD lock poisoned");
    // Two independent "callers" parse the same command string and must obtain
    // equivalent typed requests routed through the identical boundary.
    let a: Vec<String> = vec!["patch".into(), "3".into(), "--text".into(), "hello".into()];
    let b: Vec<String> = vec!["patch".into(), "3".into(), "--text".into(), "hello".into()];
    let ca = infraware::parse_args(&a).expect("caller A");
    let cb = infraware::parse_args(&b).expect("caller B");
    assert_eq!(ca, cb);
    // And both dispatch through the same seam with the same result. `init` is
    // not exercised here, so a stable sentinel root is sufficient; root is only
    // touched by filesystem-backed commands.
    let root = std::path::Path::new(".");
    assert_eq!(infraware::dispatch(ca, root), infraware::dispatch(cb, root));
}
