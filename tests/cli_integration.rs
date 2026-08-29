//! End-to-end CLI integration tests for the `definition-management` capability.
//!
//! Exercises the exact approved grammar through the binary's `run` adapter
//! (parse → dispatch → streams/status), asserting exit status and stream
//! separation per the frozen `cli-foundation` contract. Because `run` resolves
//! the repository root via `std::env::current_dir()`, each test that needs an
//! isolated store scopes the process working directory to a fresh temp dir
//! (mirroring `tests/contract.rs`'s `TempCwd` guard) so the canonical
//! `.infraware/ARCHITECTURE.md` is never created in the repository.

use infraware::{EXIT_FAILURE, EXIT_USAGE, run};
use std::fs;
use std::path::PathBuf;
use std::process::Command as ProcCommand;
use std::sync::{Mutex, MutexGuard};

/// Serializes every test that mutates the process working directory.
///
/// `run` resolves the repository root via `std::env::current_dir()`, so the
/// CWD-mutating `TempCwd` guards must not run concurrently (a parallel test
/// would otherwise observe another test's temp dir). Holding this lock for the
/// lifetime of each `TempCwd` serializes the CWD mutation while leaving the
/// per-process `binary_end_to_end_against_temp_dir` test free to run in
/// parallel.
static CWD_LOCK: Mutex<()> = Mutex::new(());

/// Build argv for `run` from string literals.
fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// Capture the (status, stdout, stderr) of `run` executed against the current
/// working directory (which the binary resolves as the repository root).
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

/// RAII guard that scopes the process working directory to a fresh, unique temp
/// directory for the lifetime of the guard, restoring the original working
/// directory on drop (including on panic).
///
/// `run` resolves `std::env::current_dir()` and writes
/// `.infraware/ARCHITECTURE.md` there. Tests must not pollute the repository, so
/// they execute under this guard. (Tests run in parallel, but each guard's temp
/// dir is unique per test, so the only shared resource — the process CWD — is
/// scoped per guard and restored on drop.)
struct TempCwd {
    _lock: MutexGuard<'static, ()>,
    original: PathBuf,
    _temp: PathBuf,
}

impl TempCwd {
    /// Acquire the CWD lock, create a unique temp dir, and make it the current
    /// directory.
    fn new() -> TempCwd {
        let _lock = CWD_LOCK.lock().expect("CWD lock poisoned");
        let original = std::env::current_dir().expect("resolve original working directory");
        let temp = std::env::temp_dir().join(format!(
            "infraware-cli-{}-{}",
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

    /// Canonical document path inside this guard's temp working directory.
    fn canonical_path(&self) -> PathBuf {
        self._temp.join(".infraware").join("ARCHITECTURE.md")
    }

    /// Read the canonical document text.
    fn canonical_text(&self) -> String {
        fs::read_to_string(self.canonical_path()).expect("canonical file should exist")
    }

    /// Seed an initialized canonical store with one definition.
    fn init_with_seed(&self) {
        let dir = self._temp.join(".infraware");
        fs::create_dir_all(&dir).unwrap();
        let content = "# Architecture\n\n<!-- infraware:definition id=\"seed\" -->initial<!-- /infraware:definition -->\n\nunrelated prose\n";
        fs::write(dir.join("ARCHITECTURE.md"), content).unwrap();
    }
}

impl Drop for TempCwd {
    fn drop(&mut self) {
        // Restore the original working directory so other tests/process state
        // are unaffected, then clean up the temp dir.
        let _ = std::env::set_current_dir(&self.original);
        let _ = fs::remove_dir_all(&self._temp);
    }
}

// ---- Accepted grammar ----

#[test]
fn get_defaults_to_markdown_on_stdout() {
    let cwd = TempCwd::new();
    cwd.init_with_seed();
    let (status, stdout, stderr) = exec(&["get", "seed"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("seed"));
    assert!(stdout.contains("initial"));
    assert!(stderr.is_empty(), "no diagnostics on success");
}

#[test]
fn get_format_json_emits_json() {
    let cwd = TempCwd::new();
    cwd.init_with_seed();
    let (status, stdout, _stderr) = exec(&["get", "seed", "--format", "json"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("\"id\": \"seed\"") || stdout.contains("\"id\":\"seed\""));
    assert!(stdout.contains("initial"));
}

#[test]
fn get_format_yaml_emits_yaml() {
    let cwd = TempCwd::new();
    cwd.init_with_seed();
    let (status, stdout, _stderr) = exec(&["get", "seed", "--format", "yaml"]);
    assert_eq!(status, 0);
    assert!(stdout.contains("id: seed"));
    assert!(stdout.contains("initial"));
}

#[test]
fn insert_adds_new_definition() {
    let cwd = TempCwd::new();
    cwd.init_with_seed();
    let (status, _stdout, _stderr) = exec(&["insert", "abc", "--value", "hello"]);
    assert_eq!(status, 0);
    let text = cwd.canonical_text();
    assert!(text.contains("id=\"abc\""));
    assert!(text.contains("hello"));
    // Unrelated content preserved.
    assert!(text.contains("unrelated prose"));
}

#[test]
fn edit_replaces_body() {
    let cwd = TempCwd::new();
    cwd.init_with_seed();
    let (status, _stdout, _stderr) = exec(&["edit", "seed", "--value", "updated"]);
    assert_eq!(status, 0);
    let text = cwd.canonical_text();
    assert!(text.contains("updated"));
    assert!(!text.contains("initial"), "old body must be gone");
    assert!(text.contains("unrelated prose"));
}

#[test]
fn remove_deletes_definition() {
    let cwd = TempCwd::new();
    cwd.init_with_seed();
    let (status, _stdout, _stderr) = exec(&["remove", "seed"]);
    assert_eq!(status, 0);
    let text = cwd.canonical_text();
    assert!(!text.contains("id=\"seed\""));
    assert!(text.contains("unrelated prose"));
}

// ---- Rejected shapes ----

#[test]
fn rejected_unknown_option_verbose() {
    let cwd = TempCwd::new();
    cwd.init_with_seed();
    let (status, stdout, stderr) = exec(&["get", "seed", "--verbose"]);
    assert_eq!(status, EXIT_USAGE);
    assert!(stdout.is_empty(), "no stdout on usage error");
    assert!(!stderr.is_empty());
}

#[test]
fn rejected_insert_without_value() {
    let cwd = TempCwd::new();
    cwd.init_with_seed();
    let (status, stdout, _stderr) = exec(&["insert", "abc"]);
    assert_eq!(status, EXIT_USAGE);
    assert!(stdout.is_empty());
    // Document unchanged.
    let text = cwd.canonical_text();
    assert!(!text.contains("id=\"abc\""));
}

#[test]
fn rejected_get_without_id() {
    let cwd = TempCwd::new();
    cwd.init_with_seed();
    let (status, stdout, _stderr) = exec(&["get"]);
    assert_eq!(status, EXIT_USAGE);
    assert!(stdout.is_empty());
}

#[test]
fn rejected_invalid_format_xml() {
    let cwd = TempCwd::new();
    cwd.init_with_seed();
    let (status, stdout, _stderr) = exec(&["get", "seed", "--format", "xml"]);
    assert_eq!(status, EXIT_USAGE);
    assert!(stdout.is_empty());
}

#[test]
fn no_set_or_rm_aliases() {
    let cwd = TempCwd::new();
    cwd.init_with_seed();
    for cmd in [
        ["set", "abc", "--value", "x"].as_slice(),
        ["rm", "abc"].as_slice(),
    ] {
        let (status, stdout, _stderr) = exec(cmd);
        assert_eq!(status, EXIT_USAGE, "alias must be unknown command");
        assert!(stdout.is_empty());
    }
}

// ---- Strict semantics & uninitialized store ----

#[test]
fn get_absent_is_nonzero_no_stdout() {
    let cwd = TempCwd::new();
    cwd.init_with_seed();
    let (status, stdout, stderr) = exec(&["get", "ghost"]);
    assert_eq!(status, EXIT_FAILURE);
    assert!(stdout.is_empty(), "no structured output on missing get");
    assert!(!stderr.is_empty());
}

#[test]
fn insert_duplicate_is_rejected() {
    let cwd = TempCwd::new();
    cwd.init_with_seed();
    let (status, stdout, _stderr) = exec(&["insert", "seed", "--value", "x"]);
    assert_eq!(status, EXIT_USAGE);
    assert!(stdout.is_empty());
    assert!(cwd.canonical_text().contains("initial"));
}

#[test]
fn edit_absent_is_rejected() {
    let cwd = TempCwd::new();
    cwd.init_with_seed();
    let (status, _stdout, _stderr) = exec(&["edit", "ghost", "--value", "x"]);
    assert_eq!(status, EXIT_FAILURE);
    assert!(cwd.canonical_text().contains("initial"));
}

#[test]
fn remove_absent_is_rejected() {
    let cwd = TempCwd::new();
    cwd.init_with_seed();
    let (status, _stdout, _stderr) = exec(&["remove", "ghost"]);
    assert_eq!(status, EXIT_FAILURE);
    assert!(cwd.canonical_text().contains("initial"));
}

#[test]
fn uninitialized_store_reports_error_no_implicit_init() {
    let cwd = TempCwd::new(); // no storage seeded
    let (status, stdout, stderr) = exec(&["get", "seed"]);
    assert_eq!(status, EXIT_FAILURE);
    assert!(stdout.is_empty());
    assert!(!stderr.is_empty());
    // Nothing was created in the temp working directory.
    assert!(!cwd.canonical_path().exists());
}

#[test]
fn get_never_writes_to_disk() {
    let cwd = TempCwd::new();
    cwd.init_with_seed();
    let before = cwd.canonical_text();
    let (_status, _stdout, _stderr) = exec(&["get", "seed"]);
    assert_eq!(
        cwd.canonical_text(),
        before,
        "get must not modify canonical file"
    );
}

/// Verify the actual compiled binary also honors the contract end-to-end.
///
/// `ProcCommand` sets the child process's working directory directly, so the
/// real binary resolves the temp dir as its repository root (no global CWD
/// mutation needed here).
#[test]
fn binary_end_to_end_against_temp_dir() {
    let base = std::env::temp_dir().join(format!(
        "infraware-cli-bin-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&base).unwrap();
    let _cleanup = CleanupDir(base.clone());

    let bin = env!("CARGO_BIN_EXE_infraware");

    // init creates storage.
    let out = ProcCommand::new(bin)
        .current_dir(&base)
        .arg("init")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(base.join(".infraware").join("ARCHITECTURE.md").exists());

    // insert + get round trip.
    let out = ProcCommand::new(bin)
        .current_dir(&base)
        .args(["insert", "x", "--value", "value-x"])
        .output()
        .unwrap();
    assert!(out.status.success());

    let out = ProcCommand::new(bin)
        .current_dir(&base)
        .args(["get", "x", "--format", "json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("value-x"));
    assert!(out.stderr.is_empty());
}

/// RAII guard that removes a directory on drop, even on panic.
struct CleanupDir(PathBuf);

impl Drop for CleanupDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
