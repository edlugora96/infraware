//! Focused v2 six-verb grammar tests.
use infraware::{Command, EXIT_USAGE, Invocation, InvocationOptions, OutputFormat, run};
fn parsed(command: Command) -> Invocation {
    Invocation {
        command,
        options: InvocationOptions {
            format: OutputFormat::Yaml,
            detailed: false,
        },
    }
}
#[test]
fn shared_options_accept_only_valid_trailing_forms() {
    for args in [
        vec!["status".into(), "--format".into(), "yaml".into()],
        vec!["status".into(), "--format".into(), "json".into()],
        vec!["status".into(), "--detailed".into()],
        vec![
            "status".into(),
            "--format".into(),
            "json".into(),
            "--detailed".into(),
        ],
    ] {
        assert!(infraware::parse_args(&args).is_ok());
    }
    for args in [
        vec!["status".into(), "--format".into()],
        vec!["status".into(), "--format".into(), "xml".into()],
        vec![
            "status".into(),
            "--format".into(),
            "json".into(),
            "--format".into(),
            "yaml".into(),
        ],
        vec!["status".into(), "--detailed".into(), "--detailed".into()],
        vec!["status".into(), "--detailed".into(), "x".into()],
    ] {
        assert!(infraware::parse_args(&args).unwrap_err().is_usage());
    }
}

#[test]
fn accepted_shapes() {
    for a in [
        vec!["query".into(), "q".into()],
        vec!["list".into()],
        vec!["get".into(), "1".into()],
        vec!["remove".into(), "1".into()],
        vec!["status".into()],
    ] {
        assert!(infraware::parse_args(&a).is_ok());
    }
    assert_eq!(
        infraware::parse_args(&["list".into(), "2".into(), "1".into()]),
        Ok(parsed(Command::List {
            limit: 2,
            offset: 1
        }))
    );
    assert_eq!(
        infraware::parse_args(&["list".into(), "999".into()]),
        Ok(parsed(Command::List {
            limit: 500,
            offset: 0
        }))
    );
    assert!(
        infraware::parse_args(&[
            "list".into(),
            "10".into(),
            "20".into(),
            "--format".into(),
            "json".into(),
            "--detailed".into(),
        ])
        .is_ok()
    );
}
#[test]
fn entity_selectors_are_scoped_to_entity_verbs() {
    for entity in ["rules", "tasks", "all", "missing"] {
        assert!(
            infraware::parse_args(&["list".into(), "--entity".into(), entity.into(),]).is_err()
        );
    }
    assert!(
        infraware::parse_args(&["get".into(), "3".into(), "--entity".into(), "tasks".into()])
            .is_ok()
    );
    assert!(
        infraware::parse_args(&[
            "patch".into(),
            "3".into(),
            "--text".into(),
            "x".into(),
            "--entity".into(),
            "tasks".into()
        ])
        .is_ok()
    );
    assert!(
        infraware::parse_args(&[
            "revise".into(),
            "3".into(),
            "--entity".into(),
            "rules".into()
        ])
        .is_ok()
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
fn status_rejects_operands_with_usage_status() {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = run(vec!["status".into(), "extra".into()], &mut out, &mut err);
    assert_eq!(code, EXIT_USAGE);
    assert!(out.is_empty());
    assert_eq!(String::from_utf8(err).unwrap().lines().count(), 1);
}

#[test]
fn malformed_list_cli_has_status_two_empty_stdout_and_one_diagnostic() {
    for args in [
        vec!["list", "--entity", "rules"],
        vec!["list", "--entity", "tasks"],
        vec!["list", "--entity", "all"],
        vec!["list", "--entity", "missing"],
        vec!["list", "x"],
        vec!["list", "1", "x"],
        vec!["list", "1", "2", "3"],
        vec!["list", "--detailed", "1"],
        vec!["list", "--format", "json", "1"],
        vec!["list", "--format", "xml"],
        vec!["list", "--bogus"],
    ] {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let status = run(
            args.iter().map(|arg| (*arg).to_string()),
            &mut out,
            &mut err,
        );
        assert_eq!(status, EXIT_USAGE, "{args:?}");
        assert!(out.is_empty(), "{args:?}");
        assert_eq!(err.iter().filter(|b| **b == b'\n').count(), 1, "{args:?}");
    }
}
