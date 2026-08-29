//! Contract (integration) tests for the thin binary adapter: stream separation,
//! exit-status mapping, and caller-independent request construction.

use infraware::{Command, OutputFormat, run};
use std::io::Cursor;
use std::path::{Path, PathBuf};

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
/// `run`'s `init` resolves `std::env::current_dir()` and writes
/// `.infraware/ARCHITECTURE.md` there. Real-binary init contract tests must not
/// pollute the repository, so they execute under this guard. (Tests run in
/// parallel, but only the init contract test performs a working-directory write;
/// all other contract assertions are independent of the working directory.)
struct TempCwd {
    original: PathBuf,
    _temp: PathBuf,
}

impl TempCwd {
    /// Create a unique temp dir and make it the current directory.
    fn new() -> TempCwd {
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

/// `init` rooted at the repository's working directory: the canonical path that
/// the real binary would create when run from `cwd`.
fn canonical_path_for(cwd: &Path) -> PathBuf {
    cwd.join(".infraware").join("ARCHITECTURE.md")
}

#[test]
fn analyze_empty_succeeds_with_status_zero_and_no_stderr() {
    // `analyze` now yields a well-formed, non-empty report body (the deferred
    // Plan 4 behavior): status 0, diagnostics on stderr only when something is
    // actually wrong. Run against the repository root (read-only scan).
    let (stdout, stderr, status) = execute(&["analyze", "--format", "json"]);
    assert_eq!(status, 0);
    assert!(stderr.is_empty(), "no diagnostic for successful analyze");
    assert!(
        !stdout.is_empty(),
        "analyze now emits a structured report body"
    );
    let out = String::from_utf8(stdout).expect("utf8 stdout");
    assert!(out.contains("\"languages\""), "report has category keys");
    assert!(out.contains("\"suggestions\""), "report has suggestion key");
}

#[test]
fn analyze_suggest_succeeds_status_zero() {
    let (_, stderr, status) = execute(&["analyze", "--suggest"]);
    assert_eq!(status, 0);
    assert!(stderr.is_empty());
}

#[test]
fn init_succeeds_status_zero() {
    // Isolate the real-binary init in a temp working directory so `cargo test`
    // never creates `.infraware/ARCHITECTURE.md` in the repository. The process
    // CWD is restored (and the temp dir removed) when the guard drops.
    let _guard = TempCwd::new();
    let (_, stderr, status) = execute(&["init"]);
    assert_eq!(status, 0);
    assert!(stderr.is_empty());
    // The canonical storage was created inside the temp working directory, not
    // the repository: assert it exists at the (temp) CWD the binary ran from.
    let created = canonical_path_for(
        std::env::current_dir()
            .expect("temp CWD is set by the guard")
            .as_path(),
    );
    assert!(
        created.exists(),
        "init created .infraware/ARCHITECTURE.md in temp CWD"
    );
}

#[test]
fn get_missing_definition_is_nonzero_and_diagnostic_on_stderr_only() {
    // Isolate from the seeded repository so storage-backed commands do not read
    // or write the canonical store under the real working directory.
    let _guard = TempCwd::new();
    let (stdout, stderr, status) = execute(&["get", "abc"]);
    assert_ne!(status, 0);
    assert!(!stderr.is_empty(), "diagnostic must be on stderr");
    assert!(stdout.is_empty(), "no structured output on missing get");
}

#[test]
fn insert_requires_storage_nonzero_stderr_only() {
    let _guard = TempCwd::new();
    let (stdout, stderr, status) = execute(&["insert", "abc", "--value", "x"]);
    assert_ne!(status, 0);
    assert!(!stderr.is_empty());
    assert!(stdout.is_empty());
}

#[test]
fn edit_requires_storage_nonzero_stderr_only() {
    let _guard = TempCwd::new();
    let (stdout, stderr, status) = execute(&["edit", "abc", "--value", "y"]);
    assert_ne!(status, 0);
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
fn invalid_format_is_usage_status() {
    let (stdout, stderr, status) = execute(&["get", "abc", "--format", "xml"]);
    assert_eq!(status, infraware::EXIT_USAGE);
    assert!(!stderr.is_empty());
    assert!(stdout.is_empty());
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
    let (stdout, stderr, status) = execute(&["init", "--force"]);
    assert_eq!(status, infraware::EXIT_USAGE);
    assert!(!stderr.is_empty());
    assert!(stdout.is_empty());
}

#[test]
fn explicit_json_format_honored_in_accepted_shape() {
    // Accepted shapes do not error; verify via the parser-level request equality.
    let args: Vec<String> = vec!["get".into(), "abc".into(), "--format".into(), "json".into()];
    let cmd = infraware::parse_args(&args).expect("accepted shape");
    assert_eq!(
        cmd,
        Command::Get {
            id: "abc".to_string(),
            format: OutputFormat::Json,
        }
    );
}

#[test]
fn caller_independent_request_construction_is_equal() {
    // Two independent "callers" parse the same command string and must obtain
    // equivalent typed requests routed through the identical boundary.
    let a: Vec<String> = vec![
        "analyze".into(),
        "--format".into(),
        "yaml".into(),
        "--suggest".into(),
    ];
    let b: Vec<String> = vec![
        "analyze".into(),
        "--format".into(),
        "yaml".into(),
        "--suggest".into(),
    ];
    let ca = infraware::parse_args(&a).expect("caller A");
    let cb = infraware::parse_args(&b).expect("caller B");
    assert_eq!(ca, cb);
    // And both dispatch through the same seam with the same result. `init` is
    // not exercised here, so a stable sentinel root is sufficient; root is only
    // touched by filesystem-backed commands.
    let root = std::path::Path::new(".");
    assert_eq!(infraware::dispatch(ca, root), infraware::dispatch(cb, root));
}
