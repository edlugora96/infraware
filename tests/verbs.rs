//! Focused v2 six-verb grammar tests.
use infraware::{Command, EXIT_USAGE, run};
#[test]
fn accepted_shapes() {
    for a in [
        vec!["query".into(), "q".into()],
        vec!["list".into()],
        vec!["get".into(), "1".into()],
        vec!["remove".into(), "1".into()],
    ] {
        assert!(infraware::parse_args(&a).is_ok());
    }
    assert_eq!(
        infraware::parse_args(&["list".into(), "2".into(), "1".into()]),
        Ok(Command::List {
            limit: 2,
            offset: 1
        })
    );
}
#[test]
fn rejected_legacy_and_bad_args() {
    for a in [
        vec!["init".into()],
        vec!["query".into()],
        vec!["get".into(), "1".into(), "x".into()],
        vec!["patch".into(), "1".into(), "--metadata".into(), "{}".into()],
    ] {
        assert!(infraware::parse_args(&a).unwrap_err().is_usage());
    }
}
#[test]
fn malformed_cli_has_status_two_empty_stdout() {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let s = run(
        vec!["list".into(), "--format".into(), "json".into()],
        &mut out,
        &mut err,
    );
    assert_eq!(s, EXIT_USAGE);
    assert!(out.is_empty());
    assert_eq!(err.iter().filter(|b| **b == b'\n').count(), 1);
}
