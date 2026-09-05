//! v2 CLI contract: exact grammar and stream/status behavior.
use infraware::{
    Command, EXIT_USAGE, Entity, Invocation, InvocationOptions, ListEntity, OutputFormat, run,
};
fn parsed(command: Command) -> Invocation {
    Invocation {
        command,
        options: InvocationOptions {
            format: OutputFormat::Yaml,
            detailed: false,
        },
    }
}
use std::io::Cursor;
fn exec(a: &[&str]) -> (Vec<u8>, Vec<u8>, u8) {
    let mut o = Cursor::new(Vec::new());
    let mut e = Cursor::new(Vec::new());
    let args: Vec<String> = a.iter().map(|x| x.to_string()).collect();
    let s = run(args, &mut o, &mut e);
    (o.into_inner(), e.into_inner(), s)
}
#[test]
fn six_verbs_parse() {
    assert_eq!(
        infraware::parse_args(&["ingest".into(), "task".into(), "rule".into()]),
        Ok(parsed(Command::Ingest {
            task_text: "task".into(),
            rule_text: "rule".into()
        }))
    );
    assert_eq!(
        infraware::parse_args(&["query".into(), "q".into()]),
        Ok(parsed(Command::Query { text: "q".into() }))
    );
    assert_eq!(
        infraware::parse_args(&["list".into()]),
        Ok(parsed(Command::List {
            limit: 50,
            offset: 0,
            entity: ListEntity::Rules
        }))
    );
    assert_eq!(
        infraware::parse_args(&["get".into(), "1".into()]),
        Ok(parsed(Command::Get {
            id: "1".into(),
            entity: Entity::Rules
        }))
    );
    assert!(
        infraware::parse_args(&["patch".into(), "1".into(), "--text".into(), "x".into()]).is_ok()
    );
    assert!(infraware::parse_args(&["remove".into(), "1".into()]).is_ok());
}
#[test]
fn invalid_shapes_are_usage_and_silent_on_stdout() {
    for a in [
        &["" as &str][..],
        &["ingest"],
        &["ingest", "a"],
        &["ingest", "a", "b", "c"],
        &["query"],
        &["list", "x", "y", "z"],
        &["get"],
        &["patch", "1"],
        &["remove"],
        &["legacy"],
    ] {
        let (o, e, s) = exec(a);
        assert_eq!(s, EXIT_USAGE);
        assert!(o.is_empty(), "{a:?}");
        assert_eq!(e.iter().filter(|b| **b == b'\n').count(), 1);
    }
}
#[test]
fn usage_does_not_touch_store() {
    let (o, _, s) = exec(&["query", " "]);
    assert_eq!(s, EXIT_USAGE);
    assert!(o.is_empty());
}
