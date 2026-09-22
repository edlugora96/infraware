use infraware::{EXIT_FAILURE, EXIT_USAGE, run};
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

fn lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn root() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "infraware-init-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn run_at(root: &Path, args: &[&str]) -> (u8, String, String) {
    let _guard = lock().lock().unwrap();
    let old = std::env::current_dir().unwrap();
    std::env::set_current_dir(root).unwrap();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let status = run(
        args.iter().map(|arg| (*arg).to_owned()),
        &mut stdout,
        &mut stderr,
    );
    std::env::set_current_dir(old).unwrap();
    (
        status,
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

#[test]
fn init_creates_empty_canonical_storage_with_exact_streams() {
    let root = root();
    let (status, stdout, stderr) = run_at(&root, &["init"]);
    assert_eq!(status, 0);
    assert_eq!(stdout, "initialized canonical storage\n");
    assert!(stderr.is_empty());

    let db = root.join(".infraware/vec-index.sqlite");
    assert!(db.is_file());
    let connection = Connection::open(db).unwrap();
    for table in [
        "tasks",
        "rules",
        "task_vecs",
        "rule_vecs",
        "task_rule_relations",
    ] {
        let count: i64 = connection
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "{table} must be empty");
    }
    let version: String = connection
        .query_row(
            "SELECT value FROM model_meta WHERE key='schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, "1");
}

#[test]
fn init_is_idempotent_and_preserves_existing_file_bytes() {
    let root = root();
    assert_eq!(run_at(&root, &["init"]).0, 0);
    let db = root.join(".infraware/vec-index.sqlite");
    let before = fs::read(&db).unwrap();
    let (status, stdout, stderr) = run_at(&root, &["init"]);
    assert_eq!(status, 0);
    assert_eq!(stdout, "initialized canonical storage\n");
    assert!(stderr.is_empty());
    assert_eq!(fs::read(db).unwrap(), before);
}

#[test]
fn init_rejects_invalid_arguments_before_storage_access() {
    let root = root();
    let (status, stdout, stderr) = run_at(&root, &["init", "extra"]);
    assert_eq!(status, EXIT_USAGE);
    assert!(stdout.is_empty());
    assert_eq!(stderr.lines().count(), 1);
    assert!(!root.join(".infraware").exists());
}

#[test]
fn missing_store_diagnostic_is_exact_and_redacted() {
    let root = root();
    let (status, stdout, stderr) = run_at(&root, &["list"]);
    assert_eq!(status, EXIT_FAILURE);
    assert!(stdout.is_empty());
    assert_eq!(
        stderr,
        "error: canonical storage is not initialized; run `infraware init` first\n"
    );
}

#[test]
fn init_does_not_replace_corrupt_or_incompatible_storage() {
    for bytes in [b"not sqlite".to_vec(), {
        let path = root();
        fs::create_dir_all(path.join(".infraware")).unwrap();
        let db = path.join(".infraware/vec-index.sqlite");
        let connection = Connection::open(&db).unwrap();
        connection
            .execute("CREATE TABLE unrelated(value TEXT)", [])
            .unwrap();
        drop(connection);
        let original = fs::read(&db).unwrap();
        let (status, stdout, stderr) = run_at(&path, &["init"]);
        assert_eq!(status, EXIT_FAILURE);
        assert!(stdout.is_empty());
        assert_eq!(stderr.lines().count(), 1);
        assert_eq!(fs::read(&db).unwrap(), original);
        original
    }] {
        let path = root();
        fs::create_dir_all(path.join(".infraware")).unwrap();
        let db = path.join(".infraware/vec-index.sqlite");
        fs::write(&db, &bytes).unwrap();
        let (status, stdout, stderr) = run_at(&path, &["init"]);
        assert_eq!(status, EXIT_FAILURE);
        assert!(stdout.is_empty());
        assert_eq!(stderr.lines().count(), 1);
        assert_eq!(fs::read(&db).unwrap(), bytes);
    }
}
