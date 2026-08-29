//! Contract (integration) tests for the thin binary adapter: stream separation,
//! exit-status mapping, and caller-independent request construction.

use infraware::{Command, OutputFormat, run};
use std::io::Cursor;

/// Run `run` against a string argv and capture stdout/stderr bytes and status.
fn execute(argv: &[&str]) -> (Vec<u8>, Vec<u8>, u8) {
    let args: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    let mut stdout = Cursor::new(Vec::new());
    let mut stderr = Cursor::new(Vec::new());
    let code = run(args, &mut stdout, &mut stderr);
    (stdout.into_inner(), stderr.into_inner(), code)
}

#[test]
fn analyze_empty_succeeds_with_status_zero_and_no_stderr() {
    let (stdout, stderr, status) = execute(&["analyze"]);
    assert_eq!(status, 0);
    assert!(stderr.is_empty(), "no diagnostic for empty analyze");
    assert!(
        stdout.is_empty(),
        "foundation analyze has no structured body"
    );
}

#[test]
fn analyze_suggest_succeeds_status_zero() {
    let (_, stderr, status) = execute(&["analyze", "--suggest"]);
    assert_eq!(status, 0);
    assert!(stderr.is_empty());
}

#[test]
fn init_succeeds_status_zero() {
    let (_, stderr, status) = execute(&["init"]);
    assert_eq!(status, 0);
    assert!(stderr.is_empty());
}

#[test]
fn get_missing_definition_is_nonzero_and_diagnostic_on_stderr_only() {
    let (stdout, stderr, status) = execute(&["get", "abc"]);
    assert_ne!(status, 0);
    assert!(!stderr.is_empty(), "diagnostic must be on stderr");
    assert!(stdout.is_empty(), "no structured output on missing get");
}

#[test]
fn insert_requires_storage_nonzero_stderr_only() {
    let (stdout, stderr, status) = execute(&["insert", "abc", "--value", "x"]);
    assert_ne!(status, 0);
    assert!(!stderr.is_empty());
    assert!(stdout.is_empty());
}

#[test]
fn edit_requires_storage_nonzero_stderr_only() {
    let (stdout, stderr, status) = execute(&["edit", "abc", "--value", "y"]);
    assert_ne!(status, 0);
    assert!(!stderr.is_empty());
    assert!(stdout.is_empty());
}

#[test]
fn remove_requires_storage_nonzero_stderr_only() {
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
    // And both dispatch through the same seam with the same result.
    assert_eq!(infraware::dispatch(ca), infraware::dispatch(cb));
}
