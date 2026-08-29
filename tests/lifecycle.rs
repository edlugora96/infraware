//! Integration tests for the canonical-document lifecycle and persistence
//! boundary (tasks 3.2, 4.1, 4.2, 4.3, 6.2). Uses isolated temp directories so
//! the repository is never touched and no artifacts are left behind.

use infraware::{
    Command, DispatchOutput, Document, LibraryError, OutputFormat, Span, canonical_path, dispatch,
    init, init_markdown, parse_document, serialize_document, write_atomic,
};
use std::fs;
use std::path::PathBuf;

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
        "infraware-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&base).expect("create temp root");
    TempRoot(base)
}

#[test]
fn init_creates_dir_and_file_with_comments_only() {
    let root = temp_root();
    init(&root.0).expect("init succeeds");
    let path = canonical_path(&root.0);
    assert!(path.exists(), "ARCHITECTURE.md created");
    assert!(path.parent().unwrap().ends_with(".infraware"));

    let content = fs::read_to_string(&path).unwrap();
    assert!(content.starts_with("# Architecture"), "has title heading");
    // No definitions, answers, or mandatory schema.
    assert!(
        !content.contains("infraware:definition id="),
        "init must not create definitions"
    );
    // Guidance appears as comments only.
    assert!(content.contains("<!--"), "guidance rendered as comments");
}

#[test]
fn init_is_idempotent_and_does_not_clobber() {
    let root = temp_root();
    init(&root.0).unwrap();
    let path = canonical_path(&root.0);
    let first = fs::read_to_string(&path).unwrap();

    // Simulate an external edit; a second init must NOT overwrite it.
    let edited = format!("{first}SENTINEL_APPENDED");
    fs::write(&path, &edited).unwrap();

    init(&root.0).expect("second init succeeds");
    let second = fs::read_to_string(&path).unwrap();
    assert_eq!(
        second, edited,
        "existing content must be left untouched (no clobber)"
    );
}

#[test]
fn atomic_write_succeeds_with_no_temp_leftover() {
    let root = temp_root();
    init(&root.0).unwrap();
    let doc =
        "# Architecture\n\n<!-- infraware:definition id=\"a\" -->x<!-- /infraware:definition -->\n";
    write_atomic(&root.0, doc).expect("atomic write succeeds");

    let path = canonical_path(&root.0);
    assert_eq!(fs::read_to_string(&path).unwrap(), doc);

    let dir = path.parent().unwrap();
    let leftovers: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "no temporary artifact left behind: {leftovers:?}"
    );
}

#[test]
fn atomic_write_rejects_malformed_and_keeps_prior() {
    let root = temp_root();
    init(&root.0).unwrap();
    let path = canonical_path(&root.0);
    let prior = fs::read_to_string(&path).unwrap();

    let bad = "<!-- infraware:definition id=\"a\" -->no closing marker";
    assert!(
        write_atomic(&root.0, bad).is_err(),
        "malformed content rejected"
    );

    // Prior content untouched and no temp leftover.
    assert_eq!(fs::read_to_string(&path).unwrap(), prior);
    let dir = path.parent().unwrap();
    let leftovers: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("tmp"))
        .collect();
    assert!(leftovers.is_empty(), "no temporary artifact after failure");
}

#[test]
fn dispatch_init_creates_storage_with_empty_body() {
    let root = temp_root();

    // ---- Dispatch-boundary wiring (Task 5.1) ----

    // `init` routed through the single caller-independent dispatch boundary
    // must create the canonical storage and return an empty structured body
    // (no stdout), preserving the frozen stream/exit contract.
    let out: DispatchOutput = dispatch(Command::Init, &root.0).expect("dispatch init succeeds");
    assert_eq!(out.format, OutputFormat::Markdown);
    assert!(
        out.body.is_empty(),
        "dispatch init emits no structured body"
    );
    let path = canonical_path(&root.0);
    assert!(path.exists(), "dispatch created ARCHITECTURE.md");
    let content = fs::read_to_string(&path).unwrap();
    assert!(
        content.starts_with("# Architecture"),
        "created file has title"
    );
}

#[test]
fn dispatch_init_is_idempotent_and_does_not_clobber() {
    let root = temp_root();
    dispatch(Command::Init, &root.0).expect("first dispatch init succeeds");
    let path = canonical_path(&root.0);
    let first = fs::read_to_string(&path).unwrap();

    // Simulate an external edit; a second dispatch must NOT overwrite it.
    let edited = format!("{first}# external edit\n");
    fs::write(&path, &edited).unwrap();

    dispatch(Command::Init, &root.0).expect("second dispatch init succeeds");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        edited,
        "dispatch init leaves existing content untouched (no clobber)"
    );
}

#[test]
fn dispatch_init_propagates_io_error() {
    // A root that is itself a file cannot host `.infraware/`, so persistence
    // returns a typed `Io` error that dispatch propagates unchanged.
    let file = std::env::temp_dir().join(format!(
        "infraware-dispatch-file-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(&file, b"not a directory").unwrap();
    let result = dispatch(Command::Init, &file);
    let _ = fs::remove_file(&file);

    assert!(
        matches!(result, Err(LibraryError::Io { .. })),
        "dispatch init propagates typed Io error: {result:?}"
    );
}

#[test]
fn init_markdown_has_title_and_comments_only() {
    let md = init_markdown();
    assert!(md.starts_with("# Architecture"), "title heading present");
    assert!(md.contains("<!--"), "guidance as comments");
    assert!(
        !md.contains("infraware:definition id="),
        "no definitions in init content"
    );
}

#[test]
fn unrelated_content_preserved_across_mutation() {
    let root = temp_root();
    init(&root.0).unwrap();
    let original = "# Title\n\nIntro prose.\n\n<!-- infraware:definition id=\"d1\" -->old<!-- /infraware:definition -->\n\nOutro.\n";
    write_atomic(&root.0, original).unwrap();

    // Mutate only the targeted definition block; unrelated spans stay put.
    let mut doc: Document = parse_document(original).unwrap();
    for span in &mut doc.spans {
        if let Span::Definition(d) = span {
            if d.id == "d1" {
                d.body = "new".to_string();
            }
        }
    }
    let updated = serialize_document(&doc);
    assert!(!updated.contains("old"), "old body replaced");
    assert!(updated.contains("new"), "new body present");
    assert!(
        updated.contains("Intro prose."),
        "unrelated prose preserved"
    );
    assert!(updated.contains("Outro."), "unrelated trailing preserved");
    assert!(updated.contains("# Title"), "unrelated heading preserved");

    write_atomic(&root.0, &updated).unwrap();
    assert_eq!(
        fs::read_to_string(canonical_path(&root.0)).unwrap(),
        updated
    );
}
