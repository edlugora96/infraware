//! Task 5.2: CLI-level redaction for `ingest` failures (subprocess variant).
//!
//! Spawns the compiled binary with child-only `.current_dir(...)` and
//! `.env("XDG_CACHE_HOME", ...)` — no test-process working-directory or
//! environment mutation, no FFI. The model-failure case points the
//! child cache at a corrupt pinned revision, so cache validation fails
//! before any download: hermetic, no network. Asserts status 1, empty
//! stdout, exactly one redacted `error:` stderr line with no filesystem
//! paths, URLs, or vector contents.

use infraware::{EXIT_FAILURE, EXIT_USAGE};
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

/// Pinned cache layout, mirroring `src/model_cache.rs`
/// (`infraware/models/<subdir>/<revision>/`). Hardcoded because the
/// constants are crate-private; a pin change fails this test loudly.
const CACHE_SUBPATH: &str =
    "infraware/models/bge-small-en-v1.5/5c38ec7c405ec4b44b94cc5a9bb96e735b38267a";

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// Fresh temp directory, removed on drop. Uniqueness comes from a
/// process-local counter plus pid plus timestamp, so parallel tests in
/// this binary never share a root.
struct TempRoot {
    path: std::path::PathBuf,
}

impl TempRoot {
    fn new() -> Self {
        let unique = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "infraware-ingest-redaction-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            unique
        ));
        fs::create_dir_all(&path).expect("create temp root");
        TempRoot { path }
    }

    /// Poisoned child cache home: the final revision dir exists with wrong
    /// bytes, so `ensure_model_files` fails in read-only validation before
    /// any download attempt.
    fn poisoned_cache_home(&self) -> std::path::PathBuf {
        let home = self.path.join("xdg");
        let rev = home.join(CACHE_SUBPATH);
        fs::create_dir_all(&rev).expect("create corrupt revision dir");
        fs::write(rev.join("config.json"), b"wrong bytes").expect("poison config");
        home
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Run the compiled binary as a subprocess with `current_dir` = the temp
/// root and, when given, a child-only `XDG_CACHE_HOME`. The test process
/// environment and working directory are never mutated.
fn exec_in(
    temp: &std::path::Path,
    cache_home: Option<&std::path::Path>,
    args: &[&str],
) -> (Option<i32>, String, String) {
    let bin = env!("CARGO_BIN_EXE_infraware");
    let mut cmd = std::process::Command::new(bin);
    cmd.current_dir(temp).args(args);
    if let Some(home) = cache_home {
        cmd.env("XDG_CACHE_HOME", home);
    }
    let out = cmd.output().expect("spawn ingest binary");
    (
        out.status.code(),
        String::from_utf8(out.stdout).expect("stdout utf8"),
        String::from_utf8(out.stderr).expect("stderr utf8"),
    )
}

#[test]
fn ingest_model_failure_is_redacted_status_1() {
    let root = TempRoot::new();
    let cache_home = root.poisoned_cache_home();
    let (code, stdout, stderr) =
        exec_in(&root.path, Some(&cache_home), &["ingest", "hello", "rule"]);
    assert_eq!(
        code,
        Some(EXIT_FAILURE as i32),
        "model failure must exit status 1"
    );
    assert!(stdout.is_empty(), "must leave stdout empty");
    assert_eq!(
        stderr, "error: i/o error: ingest failed\n",
        "exactly one redacted line, got: {stderr:?}"
    );
    // The model stage runs before storage, so this failure need not create an index.
    // The diagnostic must not expose user data or implementation details.
    let temp = root.path.to_string_lossy().into_owned();
    for leaked in [
        temp,
        "xdg".to_string(),
        "config.json".to_string(),
        "safetensors".to_string(),
        "tokenizer".to_string(),
        "download".to_string(),
        "http".to_string(),
        "vec-index".to_string(),
        ".infraware".to_string(),
        "dimension".to_string(),
        "cosine".to_string(),
        "SELECT".to_string(),
        "hello".to_string(),
        "rule".to_string(),
    ] {
        assert!(
            !stderr.contains(&leaked),
            "stderr must not leak {leaked:?}, got: {stderr:?}"
        );
    }
}

#[test]
fn ingest_blank_text_is_usage_with_empty_stdout() {
    let root = TempRoot::new();
    let (code, stdout, stderr) = exec_in(&root.path, None, &["ingest", "   ", "rule"]);
    assert_eq!(
        code,
        Some(EXIT_USAGE as i32),
        "validation failure must exit status 2"
    );
    assert!(stdout.is_empty(), "must leave stdout empty");
    assert_eq!(stderr.lines().count(), 1, "exactly one stderr line");
    assert!(stderr.starts_with("error:"), "got: {stderr:?}");
    // Validation precedes storage: no index artifacts created.
    assert!(!root.path.join(".infraware").exists());
}
