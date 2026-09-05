//! Public acceptance coverage for paired-ingest rule/mutation boundaries.
//!
//! These cases exercise the real CLI adapter and SQLite filesystem boundary.
//! Model-backed success fixtures are intentionally not fabricated here: they
//! require the pinned embedding cache and the vector extension.

use infraware::{EXIT_FAILURE, EXIT_USAGE, run};
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

static CWD_LOCK: Mutex<()> = Mutex::new(());

struct TempCwd {
    _lock: MutexGuard<'static, ()>,
    old: PathBuf,
    path: PathBuf,
}

impl TempCwd {
    fn new() -> Self {
        let lock = CWD_LOCK.lock().expect("cwd lock");
        let old = std::env::current_dir().expect("current directory");
        let path = std::env::temp_dir().join(format!(
            "infraware-rule-mutation-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("temporary directory");
        std::env::set_current_dir(&path).expect("temporary cwd");
        Self {
            _lock: lock,
            old,
            path,
        }
    }
}

impl Drop for TempCwd {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.old);
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn exec(args: &[&str]) -> (u8, String, String) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let status = run(
        args.iter().map(ToString::to_string),
        &mut stdout,
        &mut stderr,
    );
    (
        status,
        String::from_utf8(stdout).expect("stdout is utf8"),
        String::from_utf8(stderr).expect("stderr is utf8"),
    )
}

#[test]
fn zero_hit_query_on_missing_store_is_operational_and_read_only() {
    let cwd = TempCwd::new();
    let (status, stdout, stderr) = exec(&["query", "unseen text"]);
    assert_eq!(status, EXIT_FAILURE);
    assert!(stdout.is_empty());
    assert_eq!(stderr.lines().count(), 1);
    assert!(stderr.starts_with("error:"));
    assert!(!cwd.path.join(".infraware").exists());
}

#[test]
fn list_get_patch_and_remove_missing_store_are_read_only() {
    let cwd = TempCwd::new();
    for args in [
        &["list"][..],
        &["list", "10", "20"][..],
        &["get", "1"][..],
        &["patch", "1", "--text", "replacement"][..],
        &["remove", "1"][..],
    ] {
        let (status, stdout, stderr) = exec(args);
        assert_eq!(status, EXIT_FAILURE, "{args:?}");
        assert!(stdout.is_empty(), "{args:?}");
        assert_eq!(stderr.lines().count(), 1, "{args:?}");
        assert!(stderr.starts_with("error:"), "{args:?}");
    }
    assert!(!cwd.path.join(".infraware").exists());
}

#[test]
fn malformed_query_and_mutation_inputs_are_usage_and_silent() {
    for args in [
        &["query"][..],
        &["query", "text", "extra"][..],
        &["list", "bad"][..],
        &["get", "0"][..],
        &["patch", "1"][..],
        &["patch", "1", "--metadata", "{}"][..],
        &["remove", "-1"][..],
    ] {
        let (status, stdout, stderr) = exec(args);
        assert_eq!(status, EXIT_USAGE, "{args:?}");
        assert!(stdout.is_empty(), "{args:?}");
        assert_eq!(stderr.lines().count(), 1, "{args:?}");
    }
}

#[test]
fn query_output_cannot_mutate_preexisting_file_artifacts_on_invalid_input() {
    let cwd = TempCwd::new();
    let marker = cwd.path.join("marker");
    fs::write(&marker, b"before").expect("marker");
    let before = fs::read(&marker).expect("marker bytes");
    let _ = exec(&["query", "text", "{}"]);
    assert_eq!(fs::read(marker).expect("marker bytes"), before);
}
