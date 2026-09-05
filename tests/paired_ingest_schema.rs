//! Focused public-boundary acceptance coverage for paired ingest (5.2–5.4).
//!
//! Model-backed successful ingest and deterministic vector fixtures require
//! private/source hooks not exposed by the crate; those are reported rather
//! than recreated here. These tests cover the public parser and CLI gates.

use infraware::{Command, EXIT_FAILURE, EXIT_USAGE, parse_args};
use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Project(PathBuf);

impl Project {
    fn new() -> Self {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("infraware-paired-schema-{n}"));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn index(&self) -> PathBuf {
        self.0.join(".infraware/vec-index.sqlite")
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run_in(project: &Project, args: &[&str]) -> std::process::Output {
    ProcessCommand::new(env!("CARGO_BIN_EXE_infraware"))
        .current_dir(&project.0)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn paired_parser_preserves_raw_fact_and_query_text() {
    let fact = r#"{"a":1}"#;
    let query = "find it exactly";
    assert_eq!(
        parse_args(&["ingest".into(), fact.into(), query.into()]),
        Ok(Command::Ingest {
            fact_text: fact.into(),
            query_text: query.into()
        })
    );
}

#[test]
fn invalid_pair_shapes_are_status_two_silent_and_side_effect_free() {
    let project = Project::new();
    for args in [
        &["ingest"][..],
        &["ingest", "fact"][..],
        &["ingest", "fact", "query", "extra"][..],
        &["ingest", "--fact", "query"][..],
        &["ingest", "fact", "   "][..],
    ] {
        let output = run_in(&project, args);
        assert_eq!(output.status.code(), Some(EXIT_USAGE as i32));
        assert!(output.stdout.is_empty());
        assert_eq!(
            output.stderr.iter().filter(|byte| **byte == b'\n').count(),
            1
        );
    }
    assert!(!project.0.join(".infraware").exists());
}

#[test]
fn incompatible_existing_store_is_unchanged_for_non_ingest() {
    let project = Project::new();
    fs::create_dir_all(project.index().parent().unwrap()).unwrap();
    Connection::open(project.index())
        .unwrap()
        .execute_batch("CREATE TABLE legacy(id INTEGER); INSERT INTO legacy VALUES (7);")
        .unwrap();
    let before = fs::read(project.index()).unwrap();

    for args in [&["list"][..], &["query", "anything"][..], &["get", "1"][..]] {
        let output = run_in(&project, args);
        assert_eq!(output.status.code(), Some(EXIT_FAILURE as i32));
        assert!(output.stdout.is_empty());
        assert_eq!(
            output.stderr.iter().filter(|byte| **byte == b'\n').count(),
            1
        );
        assert!(output.stderr.starts_with(b"error:"));
    }
    assert_eq!(fs::read(project.index()).unwrap(), before);
}
