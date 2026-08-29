//! Integration/contract tests for the `repository-analysis` capability.
//!
//! Covers OpenSpec phase 7 (tasks 18–23): repository fixtures and contract
//! tests for detection, built-in/gitignore skips, symlink exclusion, secret
//! non-leakage across formats, fail-soft and canonical independence, format
//! parity / byte determinism, and `--suggest` mapping/dedup/order.
//!
//! All fixtures use the existing `TempRoot` RAII pattern (mirroring
//! `dispatch.rs`/`lifecycle.rs` test conventions), and every assertion reaches
//! the analyzer only through the single dispatch seam (`dispatch`) or the
//! binary adapter (`run`) — never the `pub(crate)` pipeline internals.

use infraware::{CATALOG, Command, OutputFormat, dispatch, run};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::MutexGuard;

// `Permissions::from_mode` is used by Unix-only fixtures (set_mode helpers) and
// is provided by this trait. Gated to unix so non-unix targets don't error.
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

// ---- RAII temp-root (mirrors dispatch.rs / lifecycle.rs test pattern) ----

/// RAII guard that removes its temp directory on drop, even on panic.
struct TempRoot(PathBuf);

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Create a unique, empty temp directory owned by the returned guard.
fn temp_root() -> TempRoot {
    let base = std::env::temp_dir().join(format!(
        "infraware-analysis-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&base).expect("create temp root");
    TempRoot(base)
}

/// Write a file (creating parent dirs) relative to `root`.
fn write_file(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(path, content).expect("write fixture file");
}

/// Run analysis through the single dispatch seam and return the rendered body.
///
/// Analysis is fail-soft, so a valid report (status 0 at the seam) is always
/// expected; a dispatched error here is a genuine contract violation.
fn analyze(root: &Path, format: OutputFormat, suggest: bool) -> String {
    let out = dispatch(Command::Analyze { format, suggest }, root)
        .unwrap_or_else(|e| panic!("analyze must succeed (fail-soft): {e:?}"));
    assert_eq!(out.format, format, "dispatch echoes the requested format");
    out.body
}

// ---- Fixture builders (task 7.1) ----

/// A typical multi-signal repo: manifests, languages, CI/CD, container, entry
/// point, linter, tests.
fn build_typical(root: &Path) {
    write_file(root, "Cargo.toml", "[package]\nname = \"x\"\n");
    write_file(root, "package.json", "{}\n");
    write_file(root, "src/main.rs", "fn main() {}\n");
    write_file(root, "src/lib.rs", "// lib\n");
    write_file(root, "tests/example_test.rs", "// test\n");
    write_file(root, ".github/workflows/ci.yml", "on: push\n");
    write_file(root, "Makefile", "all:\n\techo hi\n");
    write_file(root, "Dockerfile", "FROM scratch\n");
    write_file(root, ".eslintrc", "{}\n");
    write_file(root, "README.md", "# hi\n");
}

/// A repo exercising gitignore matching (negation + nested scope) and the
/// always-applied built-in skip set. Skipped/kept assertions use signal-bearing
/// paths (`.env`/`.rs`) so they are observable in approved report collections.
fn build_gitignore(root: &Path) {
    // Root .gitignore: ignore all `.env` secret-like files, but re-include
    // `keep.env` via negation. Both are signal-bearing (secret classification),
    // so the exclusion/re-inclusion is observable without a file inventory.
    write_file(root, ".gitignore", "*.env\n!keep.env\n");
    write_file(root, "secret.env", "ignored");
    write_file(root, "keep.env", "kept");
    // Nested ignore scoped to its subtree only (signal-bearing `.rs` files).
    write_file(root, "sub/.gitignore", "nested-ignored.rs\n");
    write_file(root, "sub/nested-ignored.rs", "fn main() {}\n");
    write_file(root, "sub/visible.rs", "fn main() {}\n");
    // Built-in skip dir: applied independently of any .gitignore.
    write_file(root, "target/compiled.o", "binary");
    write_file(root, "src/app.rs", "fn main() {}\n");
}

/// A repo full of secret-suspect files with realistic secret *values* that must
/// never be read or emitted.
fn build_secret(root: &Path) {
    write_file(root, ".env", "DATABASE_URL=secret_value\nAPI_KEY=abc123\n");
    write_file(root, ".env.production", "TOKEN=supersecret\n");
    write_file(root, "credentials.json", "{\"password\":\"hunter2\"}");
    write_file(root, "secret.yaml", "apikey: topsecret\n");
    write_file(root, "id_rsa", "-----BEGIN PRIVATE KEY-----\n");
    write_file(root, "server.key", "keybytes\n");
    write_file(root, "cert.pem", "pembytes\n");
    write_file(root, "auth/token.txt", "tokendata\n");
    write_file(root, "secrets/key.p12", "pkcs12bytes\n");
    write_file(root, "README.md", "no secrets here\n");
}

/// Build an unreadable subtree. Returns the locked dir so the test can restore
/// permissions before the guard drops.
fn build_unreadable(root: &Path) -> PathBuf {
    write_file(root, "visible.rs", "fn main() {}\n");
    let locked = root.join("locked");
    fs::create_dir_all(&locked).expect("create locked dir");
    write_file(root, "locked/hidden.rs", "fn secret() {}\n");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("lock dir");
    locked
}

// ---- Detection (task 7.2) ----

#[test]
fn typical_repo_detects_all_signal_groups() {
    let root = temp_root();
    build_typical(&root.0);
    let body = analyze(&root.0, OutputFormat::Json, false);

    // Manifests.
    assert!(body.contains("cargo"), "cargo manifest detected");
    assert!(body.contains("npm"), "npm manifest detected");
    // Languages.
    assert!(body.contains("Rust"), "rust language detected");
    // CI/CD.
    assert!(body.contains("github-actions"), "github-actions detected");
    // Container.
    assert!(body.contains("dockerfile"), "dockerfile detected");
    // Entry points.
    assert!(body.contains("make"), "make entry point detected");
    assert!(body.contains("cargo-bin"), "cargo-bin entry point detected");
    // Linter.
    assert!(body.contains("eslint"), "eslint detected");
    // Tests.
    assert!(body.contains("test-dir"), "test directory detected");
}

#[test]
fn empty_repo_yields_valid_report_all_keys_status_ok() {
    let root = temp_root();
    let body = analyze(&root.0, OutputFormat::Json, false);
    for key in [
        "languages",
        "manifests",
        "linters",
        "tests",
        "containers",
        "cicd",
        "entry_points",
        "sensitive_files",
        "limitations",
        "suggestions",
    ] {
        assert!(body.contains(key), "missing category key {key}");
    }
    // Every category is an explicit (possibly empty) collection, never omitted.
    assert!(
        body.contains("\"languages\":[]"),
        "languages is empty array"
    );
    assert!(
        body.contains("\"suggestions\":[]"),
        "suggestions is empty array"
    );
}

// ---- Built-in / gitignore skips (task 7.2) ----

#[test]
fn gitignore_and_builtin_skips_exclude_paths() {
    let root = temp_root();
    build_gitignore(&root.0);
    let body = analyze(&root.0, OutputFormat::Json, false);

    // gitignore: secret.env ignored (signal-bearing `.env`), keep.env
    // re-included via negation and still classified as a secret.
    assert!(
        !body.contains("secret.env"),
        "ignored *.env must not appear"
    );
    assert!(
        body.contains("keep.env"),
        "negation must re-include keep.env (secret classification)"
    );

    // Nested ignore scoped to its subtree only (signal-bearing `.rs`).
    assert!(
        !body.contains("nested-ignored.rs"),
        "nested-ignored must be skipped in sub/"
    );
    assert!(
        body.contains("sub/visible.rs"),
        "sibling of nested-ignored must be kept"
    );

    // Built-in skip applied independently of .gitignore.
    assert!(
        !body.contains("target/compiled.o"),
        "built-in skip must exclude target/ subtree"
    );
    assert!(
        body.contains("built-in skip: target"),
        "built-in skip must be recorded as a limitation"
    );

    // Real source outside skipped trees is still reported.
    assert!(body.contains("src/app.rs"), "non-skipped source kept");
}

#[test]
fn builtin_skip_applies_without_gitignore() {
    let root = temp_root();
    // No .gitignore at all: built-in skips must still apply.
    write_file(&root.0, "node_modules/dep.js", "x");
    write_file(&root.0, "src/main.rs", "fn main() {}\n");
    let body = analyze(&root.0, OutputFormat::Json, false);
    assert!(
        !body.contains("node_modules/dep.js"),
        "built-in node_modules skipped without .gitignore"
    );
    assert!(body.contains("src/main.rs"), "real source still reported");
}

// ---- Symlink exclusion (task 7.2) ----

#[cfg(unix)]
#[test]
fn symlink_to_dir_is_not_followed() {
    let root = temp_root();
    write_file(&root.0, "realdir/inside.rs", "fn main() {}\n");
    std::os::unix::fs::symlink(root.0.join("realdir"), root.0.join("linkdir"))
        .expect("create symlink");
    let body = analyze(&root.0, OutputFormat::Json, false);

    // The symlinked subtree must never be entered via the link.
    assert!(
        !body.contains("linkdir/inside.rs"),
        "symlinked subtree must not be traversed"
    );
    // The real directory is still scanned through its own path.
    assert!(
        body.contains("realdir/inside.rs"),
        "real directory still scanned"
    );
    // The symlink is recorded as a limitation, not a traversable entry.
    assert!(
        body.contains("symlink skipped"),
        "symlink must be recorded as a limitation"
    );
}

#[cfg(unix)]
#[test]
fn symlink_to_secretlike_file_is_not_classified() {
    let root = temp_root();
    write_file(&root.0, "real.rs", "fn main() {}\n");
    // A symlink whose *name* would be classified as a secret if followed.
    std::os::unix::fs::symlink(root.0.join("real.rs"), root.0.join("link.env"))
        .expect("create symlink");
    let body = analyze(&root.0, OutputFormat::Json, false);

    // The symlink must not appear as a classified (sensitive) entry. The
    // limitation reason text ("symlink skipped ...: link.env") must not match
    // the classification path patterns below.
    assert!(
        !body.contains("\"path\":\"link.env\""),
        "symlink must not be classified as a file"
    );
    assert!(
        !body.contains("`link.env`"),
        "symlink must not appear in markdown classification"
    );
    assert!(
        body.contains("real.rs"),
        "real target still scanned through its own path"
    );
}

// ---- Secret non-leakage across formats (task 7.3) ----

#[test]
fn secret_files_reported_by_path_signal_only_no_value_leak() {
    let root = temp_root();
    build_secret(&root.0);

    // Candidate secret values that must NEVER appear in any format.
    let leaked_values = [
        "secret_value",
        "abc123",
        "supersecret",
        "hunter2",
        "topsecret",
        "tokendata",
        "pkcs12bytes",
        "keybytes",
        "pembytes",
        "BEGIN PRIVATE KEY",
    ];

    for format in [
        OutputFormat::Json,
        OutputFormat::Yaml,
        OutputFormat::Markdown,
    ] {
        let body = analyze(&root.0, format, false);

        // Paths + standard labels ARE present (evidence without disclosure).
        assert!(body.contains(".env"), "{format:?}: .env path present");
        assert!(
            body.contains("env-file"),
            "{format:?}: env-file label present"
        );
        assert!(body.contains("id_rsa"), "{format:?}: id_rsa path present");
        assert!(
            body.contains("credentials.json"),
            "{format:?}: credentials path present"
        );
        assert!(body.contains("server.key"), "{format:?}: .key path present");
        assert!(body.contains("cert.pem"), "{format:?}: .pem path present");

        // No secret *value* is ever emitted.
        for value in leaked_values {
            assert!(
                !body.contains(value),
                "{format:?}: secret value leaked: {value}"
            );
        }
    }
}

// ---- Fail-soft + canonical independence (task 7.4) ----

#[test]
fn unreadable_dir_becomes_limitation_status_ok() {
    let root = temp_root();
    let locked = build_unreadable(&root.0);
    let body = analyze(&root.0, OutputFormat::Json, false);

    // Other, readable evidence is still reported (status 0 contract).
    assert!(
        body.contains("visible.rs"),
        "readable files still reported around an unreadable dir"
    );

    // Best-effort: only assert the limitation when the dir is genuinely
    // inaccessible (root users bypass Unix permission bits).
    let truly_locked = fs::read_dir(&locked).is_err();
    if truly_locked {
        assert!(
            !body.contains("hidden.rs"),
            "unreadable subtree must not be scanned"
        );
        assert!(
            body.contains("unreadable directory"),
            "unreadable dir must be recorded as a limitation"
        );
    }

    // Restore permissions so the RAII guard can clean up.
    let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
}

#[test]
fn canonical_storage_presence_does_not_change_scan() {
    let root = temp_root();
    build_typical(&root.0);
    let without = analyze(&root.0, OutputFormat::Json, true);

    // Introduce canonical storage; analysis must remain independent of it.
    write_file(
        &root.0,
        ".infraware/ARCHITECTURE.md",
        "# Architecture\n\n<!-- infraware:definition id=\"a\" -->x<!-- /infraware:definition -->\n",
    );
    let with = analyze(&root.0, OutputFormat::Json, true);

    assert_eq!(
        without, with,
        "presence of .infraware/ARCHITECTURE.md must not change scan results"
    );
}

// ---- Format parity + byte determinism (task 7.5) ----

fn assert_all_formats_contain(root: &Path, tokens: &[&str]) {
    for format in [
        OutputFormat::Json,
        OutputFormat::Yaml,
        OutputFormat::Markdown,
    ] {
        let body = analyze(root, format, false);
        for token in tokens {
            assert!(body.contains(token), "{format:?}: missing token {token}");
        }
        // Section presence per format.
        match format {
            OutputFormat::Json => assert!(body.contains("\"languages\"")),
            OutputFormat::Yaml => assert!(body.contains("languages:")),
            OutputFormat::Markdown => assert!(body.contains("## Languages")),
        }
    }
}

#[test]
fn format_parity_conveys_same_classifications() {
    let root = temp_root();
    build_typical(&root.0);
    assert_all_formats_contain(
        &root.0,
        &[
            "cargo",
            "npm",
            "Rust",
            "github-actions",
            "dockerfile",
            "make",
            "eslint",
        ],
    );
}

#[test]
fn byte_determinism_across_runs_and_formats() {
    let root = temp_root();
    build_typical(&root.0);

    for format in [
        OutputFormat::Json,
        OutputFormat::Yaml,
        OutputFormat::Markdown,
    ] {
        let a = analyze(&root.0, format, true);
        let b = analyze(&root.0, format, true);
        let c = analyze(&root.0, format, true);
        assert_eq!(a, b, "{format:?}: run 1 vs run 2 must be byte-identical");
        assert_eq!(b, c, "{format:?}: run 2 vs run 3 must be byte-identical");
    }
}

// ---- --suggest mapping / dedup / order (task 7.6) ----

/// Extract the bullet suggestion lines from a Markdown report body.
fn suggestion_lines_md(body: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut in_sugg = false;
    for line in body.lines() {
        if line.trim() == "## Suggestions" {
            in_sugg = true;
            continue;
        }
        if in_sugg {
            if line.starts_with("## ") {
                break;
            }
            if let Some(rest) = line.strip_prefix("- ") {
                lines.push(rest);
            }
        }
    }
    lines
}

#[test]
fn suggest_maps_signals_to_catalog_lines_only() {
    let root = temp_root();
    // Only cargo + github-actions signals present.
    write_file(&root.0, "Cargo.toml", "irrelevant");
    write_file(&root.0, ".github/workflows/ci.yml", "irrelevant");
    let body = analyze(&root.0, OutputFormat::Markdown, true);

    let lines = suggestion_lines_md(&body);
    assert!(
        !lines.is_empty(),
        "observed signals should yield suggestions"
    );

    // Every suggestion is a real catalog line (mapping, not arbitrary text).
    for line in &lines {
        assert!(
            CATALOG.contains(line),
            "suggestion must come from the read-only catalog: {line}"
        );
    }
}

#[test]
fn suggest_is_deduplicated_and_stable() {
    let root = temp_root();
    // Multiple manifests of different kinds exercise dedup of shared mappings.
    write_file(&root.0, "Cargo.toml", "x");
    write_file(&root.0, "package.json", "{}");
    write_file(&root.0, "pyproject.toml", "x");
    let body = analyze(&root.0, OutputFormat::Markdown, true);

    let lines = suggestion_lines_md(&body);
    let unique: HashSet<&str> = lines.iter().copied().collect();
    assert_eq!(
        lines.len(),
        unique.len(),
        "suggestions must be deduplicated: {lines:?}"
    );

    // Stable across repeated runs (order depends only on signals).
    let again_body = analyze(&root.0, OutputFormat::Markdown, true);
    let again = suggestion_lines_md(&again_body);
    assert_eq!(lines, again, "suggestion order must be stable across runs");
}

#[test]
fn suggest_order_independent_of_signal_insertion_order() {
    // Two repos with the same signal set but different arrangement yield the
    // same deduplicated, ordered suggestion list.
    let a = temp_root();
    write_file(&a.0, "Cargo.toml", "x");
    write_file(&a.0, "Dockerfile", "x");

    let b = temp_root();
    write_file(&b.0, "Dockerfile", "x");
    write_file(&b.0, "Cargo.toml", "x");

    let sa_body = analyze(&a.0, OutputFormat::Markdown, true);
    let sa = suggestion_lines_md(&sa_body);
    let sb_body = analyze(&b.0, OutputFormat::Markdown, true);
    let sb = suggestion_lines_md(&sb_body);
    assert_eq!(sa, sb, "suggestion list must not depend on discovery order");
}

// ---- Binary adapter stream/status contract (run seam) ----

/// Serializes the single global CWD mutation used by the binary-adapter test.
static CWD_LOCK: Mutex<()> = Mutex::new(());

/// RAII guard scoping the process working directory to a fresh temp dir.
struct TempCwd {
    _lock: MutexGuard<'static, ()>,
    original: PathBuf,
    _temp: PathBuf,
}

impl TempCwd {
    fn new() -> TempCwd {
        let _lock = CWD_LOCK.lock().expect("CWD lock poisoned");
        let original = std::env::current_dir().expect("resolve original CWD");
        let temp = std::env::temp_dir().join(format!(
            "infraware-analysis-cwd-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp).expect("create temp CWD");
        std::env::set_current_dir(&temp).expect("enter temp CWD");
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
        let _ = fs::remove_dir_all(&self._temp);
    }
}

#[test]
fn analyze_via_binary_adapter_emits_body_status_zero() {
    let _guard = TempCwd::new();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let status = run(
        vec![
            "analyze".to_string(),
            "--format".to_string(),
            "json".to_string(),
        ],
        &mut stdout,
        &mut stderr,
    );
    assert_eq!(status, 0, "empty/incomplete analyze succeeds with status 0");
    assert!(stderr.is_empty(), "no diagnostics on success");
    let out = String::from_utf8(stdout).expect("utf8 stdout");
    assert!(out.contains("\"languages\""), "report body on stdout");
}
