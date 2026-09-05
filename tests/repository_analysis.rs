//! Rejection guard for the removed `repository-analysis` capability.
//!
//! The legacy `analyze` verb (with `--format`/`--suggest`/`--details`) no
//! longer exists: the CLI accepts exactly the six SQLite-backed verbs
//! (`ingest`, `query`, `list`, `get`, `patch`, `remove`). The analyzer
//! module, its fixtures, and its format-parity coverage are deleted; this
//! file keeps the test target
//! compiling and pins the removal contract: every `analyze` shape exits
//! usage status 2 with empty stdout.
//!
//! Full deletion of this target lands with the legacy-test prune.

use infraware::{EXIT_USAGE, run};

/// Capture the (status, stdout, stderr) of `run` for a string argv.
fn exec(args: &[&str]) -> (u8, String, String) {
    let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let status = run(argv, &mut stdout, &mut stderr);
    (
        status,
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

#[test]
fn analyze_verb_is_rejected_with_usage_status_and_empty_stdout() {
    for argv in [
        ["analyze"].as_slice(),
        ["analyze", "--format", "json"].as_slice(),
        ["analyze", "--suggest"].as_slice(),
        ["analyze", "--format", "yaml", "--suggest"].as_slice(),
        ["analyze", "--details"].as_slice(),
    ] {
        let (status, stdout, stderr) = exec(argv);
        assert_eq!(status, EXIT_USAGE, "{argv:?} must exit usage status 2");
        assert!(stdout.is_empty(), "{argv:?} must leave stdout empty");
        assert!(!stderr.is_empty(), "{argv:?} must diagnose on stderr");
    }
}

#[test]
fn analyze_has_no_parser_shape() {
    for argv in [
        ["analyze"].as_slice(),
        ["analyze", "--format", "json"].as_slice(),
        ["analyze", "--suggest"].as_slice(),
    ] {
        let args: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        assert!(
            infraware::parse_args(&args).unwrap_err().is_usage(),
            "{argv:?} must not parse"
        );
    }
}
