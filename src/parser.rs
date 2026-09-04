//! Stdlib-only exact-argument parser.
//!
//! Accepts exactly the approved grammar and nothing else. No `clap` or any
//! external crate is used (design D7). The grammar is small and fixed, so a
//! manual scanner with explicit rejection is the simplest correct approach.

use std::collections::BTreeMap;

use crate::command::Command;
use crate::error::LibraryError;
use crate::format::OutputFormat;

/// Parse an argv slice (excluding the program name) into a typed [`Command`].
///
/// Returns [`LibraryError::InvalidInput`] for any grammar violation: unknown
/// command, unknown option, malformed shape, or invalid `--format` value. The
/// `set`/`rm` aliases are treated as unknown commands and rejected.
pub fn parse_args(argv: &[String]) -> Result<Command, LibraryError> {
    let mut iter = argv.iter();

    let Some(command_token) = iter.next() else {
        return Err(LibraryError::InvalidInput {
            message: "no command provided".to_string(),
        });
    };

    match command_token.as_str() {
        "init" => parse_init(&mut iter),
        "analyze" => parse_analyze(&mut iter),
        "get" => parse_get(&mut iter),
        "insert" => parse_insert(&mut iter),
        "edit" => parse_edit(&mut iter),
        "remove" => parse_remove(&mut iter),
        "list" => parse_list(&mut iter),
        "ingest" => parse_ingest(&mut iter),
        "query" => parse_query(&mut iter),
        // `set`/`rm` and any other token are unknown commands, never aliases.
        other => Err(LibraryError::InvalidInput {
            message: format!("unknown command: {other}"),
        }),
    }
}

/// Parse `init` — takes no arguments.
fn parse_init<'a, I>(iter: &mut I) -> Result<Command, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    if let Some(extra) = iter.next() {
        return Err(LibraryError::InvalidInput {
            message: format!("init takes no arguments, found: {extra}"),
        });
    }
    Ok(Command::Init)
}

/// Parse `analyze [--format markdown|json|yaml] [--suggest] [--details]`.
/// Flags are order-independent and combinable; unknown flags are rejected.
fn parse_analyze<'a, I>(iter: &mut I) -> Result<Command, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    let mut format = OutputFormat::Markdown;
    let mut suggest = false;
    let mut details = false;
    let mut saw_format = false;

    while let Some(token) = iter.next() {
        match token.as_str() {
            "--format" => {
                if saw_format {
                    return Err(LibraryError::InvalidInput {
                        message: "duplicate --format option".to_string(),
                    });
                }
                saw_format = true;
                let value = iter.next().ok_or_else(|| LibraryError::InvalidInput {
                    message: "--format requires a value".to_string(),
                })?;
                format = OutputFormat::parse(value).ok_or_else(|| LibraryError::InvalidInput {
                    message: format!("invalid --format value: {value}"),
                })?;
            }
            "--suggest" => {
                if suggest {
                    return Err(LibraryError::InvalidInput {
                        message: "duplicate --suggest option".to_string(),
                    });
                }
                suggest = true;
            }
            "--details" => {
                if details {
                    return Err(LibraryError::InvalidInput {
                        message: "duplicate --details option".to_string(),
                    });
                }
                details = true;
            }
            other => {
                return Err(LibraryError::InvalidInput {
                    message: format!("unknown option for analyze: {other}"),
                });
            }
        }
    }

    Ok(Command::Analyze {
        format,
        suggest,
        details,
    })
}

/// Parse `get <ID> [--format markdown|json|yaml]`.
fn parse_get<'a, I>(iter: &mut I) -> Result<Command, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    let id = iter.next().ok_or_else(|| LibraryError::InvalidInput {
        message: "get requires an <ID>".to_string(),
    })?;

    let format = match iter.next() {
        None => OutputFormat::Markdown,
        Some(next) => match next.as_str() {
            "--format" => {
                let value = iter.next().ok_or_else(|| LibraryError::InvalidInput {
                    message: "--format requires a value".to_string(),
                })?;
                OutputFormat::parse(value).ok_or_else(|| LibraryError::InvalidInput {
                    message: format!("invalid --format value: {value}"),
                })?
            }
            other => {
                return Err(LibraryError::InvalidInput {
                    message: format!("unknown option for get: {other}"),
                });
            }
        },
    };

    if iter.next().is_some() {
        return Err(LibraryError::InvalidInput {
            message: "unexpected extra argument for get".to_string(),
        });
    }

    Ok(Command::Get {
        id: id.clone(),
        format,
    })
}

/// Parse `insert <ID> --value <TEXT>`.
fn parse_insert<'a, I>(iter: &mut I) -> Result<Command, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    parse_id_value(iter, CommandKind::Insert)
}

/// Parse `edit <ID> --value <TEXT>`.
fn parse_edit<'a, I>(iter: &mut I) -> Result<Command, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    parse_id_value(iter, CommandKind::Edit)
}

enum CommandKind {
    Insert,
    Edit,
}

/// Shared parser for `<ID> --value <TEXT>` commands.
fn parse_id_value<'a, I>(iter: &mut I, kind: CommandKind) -> Result<Command, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    let command_name = match kind {
        CommandKind::Insert => "insert",
        CommandKind::Edit => "edit",
    };

    let id = iter.next().ok_or_else(|| LibraryError::InvalidInput {
        message: format!("{command_name} requires an <ID>"),
    })?;

    let value = parse_value_option(iter, command_name)?;

    let command = match kind {
        CommandKind::Insert => Command::Insert {
            id: id.clone(),
            value,
        },
        CommandKind::Edit => Command::Edit {
            id: id.clone(),
            value,
        },
    };
    Ok(command)
}

/// Consume exactly `--value <TEXT>` and return the text.
fn parse_value_option<'a, I>(iter: &mut I, command_name: &str) -> Result<String, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    let option = iter.next().ok_or_else(|| LibraryError::InvalidInput {
        message: format!("{command_name} requires --value <TEXT>"),
    })?;

    if option.as_str() != "--value" {
        return Err(LibraryError::InvalidInput {
            message: format!("{command_name} requires --value <TEXT>, found: {option}"),
        });
    }

    let value = iter.next().ok_or_else(|| LibraryError::InvalidInput {
        message: "--value requires a <TEXT> value".to_string(),
    })?;

    if iter.next().is_some() {
        return Err(LibraryError::InvalidInput {
            message: format!("unexpected extra argument for {command_name}"),
        });
    }

    Ok(value.clone())
}

/// Parse `list [--format markdown|json|yaml]`.
fn parse_list<'a, I>(iter: &mut I) -> Result<Command, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    let mut format = OutputFormat::Markdown;
    let mut saw_format = false;
    while let Some(token) = iter.next() {
        match token.as_str() {
            "--format" => {
                if saw_format {
                    return Err(LibraryError::InvalidInput {
                        message: "duplicate --format option".to_string(),
                    });
                }
                saw_format = true;
                let value = iter.next().ok_or_else(|| LibraryError::InvalidInput {
                    message: "--format requires a value".to_string(),
                })?;
                format = OutputFormat::parse(value).ok_or_else(|| LibraryError::InvalidInput {
                    message: format!("invalid --format value: {value}"),
                })?;
            }
            other => {
                return Err(LibraryError::InvalidInput {
                    message: format!("unknown option for list: {other}"),
                });
            }
        }
    }
    Ok(Command::List { format })
}

/// Parse `ingest <TEXT> [<METADATA-JSON>]` — exactly 1-2 positional args, no flags.
fn parse_ingest<'a, I>(iter: &mut I) -> Result<Command, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    let text = iter.next().ok_or_else(|| LibraryError::InvalidInput {
        message: "ingest requires <TEXT>".to_string(),
    })?;
    if text.starts_with("--") {
        return Err(LibraryError::InvalidInput {
            message: format!("unknown option for ingest: {text}"),
        });
    }
    let metadata_raw = match iter.next() {
        None => None,
        Some(second) => {
            if second.starts_with("--") {
                return Err(LibraryError::InvalidInput {
                    message: format!("unknown option for ingest: {second}"),
                });
            }
            Some(second.clone())
        }
    };
    if let Some(extra) = iter.next() {
        return Err(LibraryError::InvalidInput {
            message: format!("unexpected extra argument for ingest: {extra}"),
        });
    }
    Ok(Command::Ingest {
        text: text.clone(),
        metadata_raw,
    })
}

/// Parse `query <TEXT> [<FILTER-JSON>]` — exactly 1-2 positional args, no flags.
///
/// Text must be non-blank but is otherwise preserved byte-exactly (no trim).
/// An absent filter defaults to `{}`. A present filter is validated through
/// the existing metadata canonicalization path and returned as the owned
/// canonical `BTreeMap` required by [`Command::Query`]. All argv and filter
/// validation happens here, before any DB/model work.
fn parse_query<'a, I>(iter: &mut I) -> Result<Command, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    let text = iter.next().ok_or_else(|| LibraryError::InvalidInput {
        message: "query requires <TEXT>".to_string(),
    })?;
    if text.starts_with("--") {
        return Err(LibraryError::InvalidInput {
            message: format!("unknown option for query: {text}"),
        });
    }
    if text.trim().is_empty() {
        return Err(LibraryError::InvalidInput {
            message: "query text must not be empty".to_string(),
        });
    }
    let filter_raw: Option<&str> = match iter.next() {
        None => None,
        Some(second) => {
            if second.starts_with("--") {
                return Err(LibraryError::InvalidInput {
                    message: format!("unknown option for query: {second}"),
                });
            }
            Some(second.as_str())
        }
    };
    if let Some(extra) = iter.next() {
        return Err(LibraryError::InvalidInput {
            message: format!("unexpected extra argument for query: {extra}"),
        });
    }
    // Validate through the existing canonicalization path; maps every filter
    // shape violation (malformed, array, nested, non-string, duplicate,
    // bad key) to `InvalidInput` before any DB/model work.
    let canonical = crate::metadata::canonicalize_metadata(filter_raw)?;
    let filter: BTreeMap<String, String> = if canonical == "{}" {
        BTreeMap::new()
    } else {
        serde_json::from_str(&canonical).map_err(|e| LibraryError::InvalidInput {
            message: format!("failed to parse canonical filter: {e}"),
        })?
    };
    Ok(Command::Query {
        text: text.clone(),
        filter,
    })
}

/// Parse `remove <ID>`.
fn parse_remove<'a, I>(iter: &mut I) -> Result<Command, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    let id = iter.next().ok_or_else(|| LibraryError::InvalidInput {
        message: "remove requires an <ID>".to_string(),
    })?;

    if iter.next().is_some() {
        return Err(LibraryError::InvalidInput {
            message: "unexpected extra argument for remove".to_string(),
        });
    }

    Ok(Command::Remove { id: id.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_init_succeeds() {
        assert_eq!(parse_args(&args(&["init"])), Ok(Command::Init));
    }

    #[test]
    fn parse_analyze_defaults_markdown() {
        assert_eq!(
            parse_args(&args(&["analyze"])),
            Ok(Command::Analyze {
                format: OutputFormat::Markdown,
                suggest: false,
                details: false,
            })
        );
    }

    #[test]
    fn parse_analyze_format_json() {
        assert_eq!(
            parse_args(&args(&["analyze", "--format", "json"])),
            Ok(Command::Analyze {
                format: OutputFormat::Json,
                suggest: false,
                details: false,
            })
        );
    }

    #[test]
    fn parse_analyze_suggest() {
        assert_eq!(
            parse_args(&args(&["analyze", "--suggest"])),
            Ok(Command::Analyze {
                format: OutputFormat::Markdown,
                suggest: true,
                details: false,
            })
        );
    }

    #[test]
    fn parse_analyze_format_yaml_suggest() {
        assert_eq!(
            parse_args(&args(&["analyze", "--format", "yaml", "--suggest"])),
            Ok(Command::Analyze {
                format: OutputFormat::Yaml,
                suggest: true,
                details: false,
            })
        );
    }

    #[test]
    fn parse_get_defaults_markdown() {
        assert_eq!(
            parse_args(&args(&["get", "abc"])),
            Ok(Command::Get {
                id: "abc".to_string(),
                format: OutputFormat::Markdown,
            })
        );
    }

    #[test]
    fn parse_get_format_json() {
        assert_eq!(
            parse_args(&args(&["get", "abc", "--format", "json"])),
            Ok(Command::Get {
                id: "abc".to_string(),
                format: OutputFormat::Json,
            })
        );
    }

    #[test]
    fn parse_insert_value() {
        assert_eq!(
            parse_args(&args(&["insert", "abc", "--value", "x"])),
            Ok(Command::Insert {
                id: "abc".to_string(),
                value: "x".to_string(),
            })
        );
    }

    #[test]
    fn parse_edit_value() {
        assert_eq!(
            parse_args(&args(&["edit", "abc", "--value", "y"])),
            Ok(Command::Edit {
                id: "abc".to_string(),
                value: "y".to_string(),
            })
        );
    }

    #[test]
    fn parse_remove() {
        assert_eq!(
            parse_args(&args(&["remove", "abc"])),
            Ok(Command::Remove {
                id: "abc".to_string(),
            })
        );
    }

    // ---- Rejected shapes ----

    #[test]
    fn reject_empty() {
        assert!(parse_args(&args(&[])).is_err());
    }

    #[test]
    fn reject_unknown_command() {
        assert!(parse_args(&args(&["foo"])).is_err());
        assert!(parse_args(&args(&["status"])).is_err());
        assert!(parse_args(&args(&["delete"])).is_err());
    }

    #[test]
    fn reject_set_alias() {
        assert!(parse_args(&args(&["set", "abc", "--value", "x"])).is_err());
    }

    #[test]
    fn reject_rm_alias() {
        assert!(parse_args(&args(&["rm", "abc"])).is_err());
    }

    #[test]
    fn reject_unknown_option_on_get() {
        assert!(parse_args(&args(&["get", "abc", "--verbose"])).is_err());
    }

    #[test]
    fn reject_unknown_option_on_init() {
        assert!(parse_args(&args(&["init", "--force"])).is_err());
    }

    #[test]
    fn reject_get_without_id() {
        assert!(parse_args(&args(&["get"])).is_err());
    }

    #[test]
    fn reject_insert_without_value() {
        assert!(parse_args(&args(&["insert", "abc"])).is_err());
    }

    #[test]
    fn reject_insert_without_id() {
        assert!(parse_args(&args(&["insert", "--value", "x"])).is_err());
    }

    #[test]
    fn reject_invalid_format_xml() {
        assert!(parse_args(&args(&["get", "abc", "--format", "xml"])).is_err());
        assert!(parse_args(&args(&["analyze", "--format", "xml"])).is_err());
    }

    #[test]
    fn reject_extra_argument_get() {
        assert!(parse_args(&args(&["get", "abc", "extra"])).is_err());
    }

    #[test]
    fn parse_list_defaults_markdown() {
        assert_eq!(
            parse_args(&args(&["list"])),
            Ok(Command::List {
                format: OutputFormat::Markdown
            })
        );
    }

    #[test]
    fn parse_list_format_json() {
        assert_eq!(
            parse_args(&args(&["list", "--format", "json"])),
            Ok(Command::List {
                format: OutputFormat::Json
            })
        );
    }

    #[test]
    fn parse_list_format_yaml() {
        assert_eq!(
            parse_args(&args(&["list", "--format", "yaml"])),
            Ok(Command::List {
                format: OutputFormat::Yaml
            })
        );
    }

    #[test]
    fn reject_list_details() {
        assert!(parse_args(&args(&["list", "--details"])).is_err());
    }

    #[test]
    fn reject_list_unknown_option() {
        assert!(parse_args(&args(&["list", "--verbose"])).is_err());
        assert!(parse_args(&args(&["list", "--suggest"])).is_err());
    }

    #[test]
    fn reject_list_duplicate_format() {
        assert!(parse_args(&args(&["list", "--format", "json", "--format", "yaml"])).is_err());
    }

    #[test]
    fn reject_list_invalid_format() {
        assert!(parse_args(&args(&["list", "--format", "xml"])).is_err());
    }

    #[test]
    fn reject_list_extra_positional() {
        assert!(parse_args(&args(&["list", "extra"])).is_err());
    }

    #[test]
    fn reject_add_command() {
        assert!(parse_args(&args(&["add", "x", "--value", "y"])).is_err());
    }

    #[test]
    fn parse_ingest_text_only() {
        assert_eq!(
            parse_args(&args(&["ingest", "hello"])),
            Ok(Command::Ingest {
                text: "hello".to_string(),
                metadata_raw: None,
            })
        );
    }

    #[test]
    fn parse_ingest_with_metadata() {
        assert_eq!(
            parse_args(&args(&["ingest", "hello", "{\"k\":\"v\"}"])),
            Ok(Command::Ingest {
                text: "hello".to_string(),
                metadata_raw: Some("{\"k\":\"v\"}".to_string()),
            })
        );
    }

    #[test]
    fn reject_ingest_missing_text() {
        assert_eq!(
            parse_args(&args(&["ingest"])),
            Err(LibraryError::InvalidInput {
                message: "ingest requires <TEXT>".to_string(),
            })
        );
    }

    #[test]
    fn reject_ingest_third_argument() {
        assert!(parse_args(&args(&["ingest", "a", "b", "c"])).is_err());
    }

    #[test]
    fn reject_ingest_flag_first() {
        assert_eq!(
            parse_args(&args(&["ingest", "--help"])),
            Err(LibraryError::InvalidInput {
                message: "unknown option for ingest: --help".to_string(),
            })
        );
    }

    #[test]
    fn reject_ingest_flag_second() {
        assert_eq!(
            parse_args(&args(&["ingest", "hello", "--format"])),
            Err(LibraryError::InvalidInput {
                message: "unknown option for ingest: --format".to_string(),
            })
        );
    }

    // ---- query (task 2.2): exact `query <text> [<filter-json>]` ----

    fn query_filter(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn parse_query_text_only_defaults_empty_filter() {
        assert_eq!(
            parse_args(&args(&["query", "hello"])),
            Ok(Command::Query {
                text: "hello".to_string(),
                filter: BTreeMap::new(),
            })
        );
    }

    #[test]
    fn parse_query_with_filter_canonical_map() {
        assert_eq!(
            parse_args(&args(&["query", "hello", r#"{"b":"2","a":"1"}"#])),
            Ok(Command::Query {
                text: "hello".to_string(),
                filter: query_filter(&[("a", "1"), ("b", "2")]),
            })
        );
    }

    #[test]
    fn parse_query_preserves_whitespace_bytes() {
        assert_eq!(
            parse_args(&args(&["query", "  hello world  "])),
            Ok(Command::Query {
                text: "  hello world  ".to_string(),
                filter: BTreeMap::new(),
            })
        );
    }

    #[test]
    fn parse_query_explicit_empty_object_is_absent() {
        assert_eq!(
            parse_args(&args(&["query", "hello", "{}"])),
            Ok(Command::Query {
                text: "hello".to_string(),
                filter: BTreeMap::new(),
            })
        );
    }

    #[test]
    fn reject_query_missing_text() {
        assert_eq!(
            parse_args(&args(&["query"])),
            Err(LibraryError::InvalidInput {
                message: "query requires <TEXT>".to_string(),
            })
        );
    }

    #[test]
    fn reject_query_blank_text() {
        assert!(parse_args(&args(&["query", "   "])).is_err());
    }

    #[test]
    fn reject_query_empty_text() {
        assert!(parse_args(&args(&["query", ""])).is_err());
    }

    #[test]
    fn reject_query_malformed_filter() {
        assert!(parse_args(&args(&["query", "hello", "{bad}"])).is_err());
    }

    #[test]
    fn reject_query_array_filter() {
        assert!(parse_args(&args(&["query", "hello", r#"["a"]"#])).is_err());
    }

    #[test]
    fn reject_query_nested_filter() {
        assert!(parse_args(&args(&["query", "hello", r#"{"a":{"b":"c"}}"#])).is_err());
    }

    #[test]
    fn reject_query_non_string_filter() {
        assert!(parse_args(&args(&["query", "hello", r#"{"a":1}"#])).is_err());
    }

    #[test]
    fn reject_query_duplicate_filter_keys() {
        assert!(parse_args(&args(&["query", "hello", r#"{"a":"1","a":"2"}"#])).is_err());
    }

    #[test]
    fn reject_query_bad_key_filter() {
        assert!(parse_args(&args(&["query", "hello", r#"{"a b":"1"}"#])).is_err());
    }

    #[test]
    fn reject_query_unquoted_filter() {
        assert!(parse_args(&args(&["query", "hello", r#"{a:"1"}"#])).is_err());
    }

    #[test]
    fn reject_query_flag_second_positional() {
        assert_eq!(
            parse_args(&args(&["query", "a", "--json"])),
            Err(LibraryError::InvalidInput {
                message: "unknown option for query: --json".to_string(),
            })
        );
    }

    #[test]
    fn reject_query_flag_first_positional() {
        assert_eq!(
            parse_args(&args(&["query", "--help"])),
            Err(LibraryError::InvalidInput {
                message: "unknown option for query: --help".to_string(),
            })
        );
    }

    #[test]
    fn reject_query_extra_positional() {
        assert!(parse_args(&args(&["query", "a", "{}", "extra"])).is_err());
    }
}
