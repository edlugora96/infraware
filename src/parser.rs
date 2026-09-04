//! Stdlib-only exact-argument parser.
//!
//! Accepts exactly the approved six-verb grammar and nothing else. No `clap`
//! or any external crate is used (design D7). Removed legacy verbs (`init`,
//! `analyze`, `insert`, `edit`), aliases (`set`, `rm`), and removed legacy
//! flags (`--format`, `--suggest`) are rejected as usage errors.

use std::collections::BTreeMap;

use crate::command::Command;
use crate::error::LibraryError;

/// Parse an argv slice (excluding the program name) into a typed [`Command`].
///
/// Returns [`LibraryError::InvalidInput`] for any grammar violation: unknown
/// command, unknown option, or malformed shape. Only the six SQLite-backed
/// verbs (`ingest`, `query`, `list`, `get`, `patch`, `remove`) are accepted;
/// legacy verbs (`init`, `analyze`, `insert`, `edit`), aliases (`set`, `rm`),
/// and removed legacy flags (`--format`, `--suggest`) are rejected.
pub fn parse_args(argv: &[String]) -> Result<Command, LibraryError> {
    let mut iter = argv.iter();

    let Some(command_token) = iter.next() else {
        return Err(LibraryError::InvalidInput {
            message: "no command provided".to_string(),
        });
    };

    match command_token.as_str() {
        "ingest" => parse_ingest(&mut iter),
        "query" => parse_query(&mut iter),
        "list" => parse_list(&mut iter),
        "get" => parse_get(&mut iter),
        "patch" => parse_patch(&mut iter),
        "remove" => parse_remove(&mut iter),
        // Legacy verbs, aliases, and any other token are unknown commands.
        other => Err(LibraryError::InvalidInput {
            message: format!("unknown command: {other}"),
        }),
    }
}

/// Parse `get <ID>` — exactly one positional id, no flags.
///
/// `--format` is a removed legacy option and is rejected as an unknown
/// option, as is every other flag.
fn parse_get<'a, I>(iter: &mut I) -> Result<Command, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    let id = iter.next().ok_or_else(|| LibraryError::InvalidInput {
        message: "get requires an <ID>".to_string(),
    })?;

    if let Some(extra) = iter.next() {
        if extra.starts_with("--") {
            return Err(LibraryError::InvalidInput {
                message: format!("unknown option for get: {extra}"),
            });
        }
        return Err(LibraryError::InvalidInput {
            message: "unexpected extra argument for get".to_string(),
        });
    }

    Ok(Command::Get { id: id.clone() })
}

/// Default page size for `list` when no limit positional is given.
pub(crate) const LIST_DEFAULT_LIMIT: usize = 50;
/// Maximum page size for `list`; larger limits clamp to this value.
pub(crate) const LIST_MAX_LIMIT: usize = 500;

/// Parse a `list` limit/offset positional: ASCII digits only, fits `u64`.
fn parse_list_count(raw: &str, kind: &str) -> Result<usize, LibraryError> {
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(LibraryError::InvalidInput {
            message: format!("list {kind} must be a non-negative integer, found: {raw}"),
        });
    }
    let value: u64 = raw.parse().map_err(|_| LibraryError::InvalidInput {
        message: format!("list {kind} must be a non-negative integer, found: {raw}"),
    })?;
    Ok(usize::try_from(value).unwrap_or(usize::MAX))
}

/// Parse `list [FILTER] [LIMIT] [OFFSET]` — up to three positionals, no flags.
///
/// The optional filter is validated through the existing metadata
/// canonicalization path (same conventions as `query`) and returned as the
/// owned canonical map. Limit defaults to 50 and clamps to max 500; offset
/// defaults to 0. `--format` is a removed legacy option and is rejected
/// as an unknown option, as is every other flag.
fn parse_list<'a, I>(iter: &mut I) -> Result<Command, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    let owned: Vec<String> = iter.cloned().collect();
    for token in &owned {
        if token.starts_with("--") {
            return Err(LibraryError::InvalidInput {
                message: format!("unknown option for list: {token}"),
            });
        }
    }
    if owned.len() > 3 {
        return Err(LibraryError::InvalidInput {
            message: format!("unexpected extra argument for list: {}", owned[3]),
        });
    }
    let filter_raw: Option<&str> = owned.first().map(String::as_str);
    let canonical = crate::metadata::canonicalize_metadata(filter_raw)?;
    let filter: BTreeMap<String, String> = if canonical == "{}" {
        BTreeMap::new()
    } else {
        serde_json::from_str(&canonical).map_err(|e| LibraryError::InvalidInput {
            message: format!("failed to parse canonical filter: {e}"),
        })?
    };
    let mut limit = LIST_DEFAULT_LIMIT;
    if let Some(raw) = owned.get(1) {
        limit = parse_list_count(raw, "limit")?.min(LIST_MAX_LIMIT);
    }
    let mut offset = 0usize;
    if let Some(raw) = owned.get(2) {
        offset = parse_list_count(raw, "offset")?;
    }
    Ok(Command::List {
        filter,
        limit,
        offset,
    })
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

/// Parse `patch <ID> [--text <TEXT>] [--metadata <JSON>]`.
///
/// `<ID>` is required and carried raw (positive-integer validation happens
/// at execution alongside payload validation). `--text` and `--metadata`
/// are order-independent and each accepted at most once; at least one must
/// be present. Unknown flags, duplicate flags, missing flag values, bare
/// extra positionals, and absent payload are all usage errors. Flag values
/// are preserved verbatim (no `--`-prefix rejection: text is byte-exact).
fn parse_patch<'a, I>(iter: &mut I) -> Result<Command, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    let id = iter.next().ok_or_else(|| LibraryError::InvalidInput {
        message: "patch requires an <ID>".to_string(),
    })?;
    let mut text: Option<String> = None;
    let mut metadata_raw: Option<String> = None;
    while let Some(token) = iter.next() {
        match token.as_str() {
            "--text" => {
                if text.is_some() {
                    return Err(LibraryError::InvalidInput {
                        message: "duplicate --text option".to_string(),
                    });
                }
                let value = iter.next().ok_or_else(|| LibraryError::InvalidInput {
                    message: "--text requires a value".to_string(),
                })?;
                text = Some(value.clone());
            }
            "--metadata" => {
                if metadata_raw.is_some() {
                    return Err(LibraryError::InvalidInput {
                        message: "duplicate --metadata option".to_string(),
                    });
                }
                let value = iter.next().ok_or_else(|| LibraryError::InvalidInput {
                    message: "--metadata requires a value".to_string(),
                })?;
                metadata_raw = Some(value.clone());
            }
            other => {
                return Err(LibraryError::InvalidInput {
                    message: format!("unknown option for patch: {other}"),
                });
            }
        }
    }
    if text.is_none() && metadata_raw.is_none() {
        return Err(LibraryError::InvalidInput {
            message: "patch requires --text or --metadata".to_string(),
        });
    }
    Ok(Command::Patch {
        id: id.clone(),
        text,
        metadata_raw,
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
    fn six_verbs_parse() {
        assert!(parse_args(&args(&["ingest", "hello"])).is_ok());
        assert!(parse_args(&args(&["query", "hello"])).is_ok());
        assert!(parse_args(&args(&["list"])).is_ok());
        assert!(parse_args(&args(&["get", "1"])).is_ok());
        assert!(parse_args(&args(&["patch", "1", "--text", "x"])).is_ok());
        assert!(parse_args(&args(&["remove", "1"])).is_ok());
    }

    #[test]
    fn reject_legacy_init() {
        assert!(parse_args(&args(&["init"])).is_err());
    }

    #[test]
    fn reject_legacy_analyze() {
        assert!(parse_args(&args(&["analyze"])).is_err());
        assert!(parse_args(&args(&["analyze", "--format", "json"])).is_err());
        assert!(parse_args(&args(&["analyze", "--suggest"])).is_err());
    }

    #[test]
    fn reject_legacy_insert() {
        assert!(parse_args(&args(&["insert", "abc", "--value", "x"])).is_err());
    }

    #[test]
    fn reject_legacy_edit() {
        assert!(parse_args(&args(&["edit", "abc", "--value", "y"])).is_err());
    }

    #[test]
    fn parse_get_id_only() {
        assert_eq!(
            parse_args(&args(&["get", "3"])),
            Ok(Command::Get {
                id: "3".to_string(),
            })
        );
    }

    #[test]
    fn reject_get_format_flag() {
        assert!(parse_args(&args(&["get", "3", "--format", "json"])).is_err());
    }

    #[test]
    fn reject_get_suggest_flag() {
        assert!(parse_args(&args(&["get", "3", "--suggest"])).is_err());
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

    #[test]
    fn parse_patch_text_only() {
        assert_eq!(
            parse_args(&args(&["patch", "3", "--text", "hello"])),
            Ok(Command::Patch {
                id: "3".to_string(),
                text: Some("hello".to_string()),
                metadata_raw: None,
            })
        );
    }

    #[test]
    fn parse_patch_metadata_only() {
        assert_eq!(
            parse_args(&args(&["patch", "3", "--metadata", r#"{"a":"1"}"#])),
            Ok(Command::Patch {
                id: "3".to_string(),
                text: None,
                metadata_raw: Some(r#"{"a":"1"}"#.to_string()),
            })
        );
    }

    #[test]
    fn parse_patch_both_flags_any_order() {
        let flags_first = parse_args(&args(&[
            "patch",
            "3",
            "--metadata",
            "{}",
            "--text",
            "hello",
        ]))
        .expect("flags-first patch parses");
        let flags_last = parse_args(&args(&[
            "patch",
            "3",
            "--text",
            "hello",
            "--metadata",
            "{}",
        ]))
        .expect("flags-last patch parses");
        assert_eq!(flags_first, flags_last);
        assert_eq!(
            flags_first,
            Command::Patch {
                id: "3".to_string(),
                text: Some("hello".to_string()),
                metadata_raw: Some("{}".to_string()),
            }
        );
    }

    #[test]
    fn parse_patch_preserves_text_bytes() {
        assert_eq!(
            parse_args(&args(&["patch", "3", "--text", "  hello world  "])),
            Ok(Command::Patch {
                id: "3".to_string(),
                text: Some("  hello world  ".to_string()),
                metadata_raw: None,
            })
        );
    }

    #[test]
    fn reject_patch_missing_id() {
        assert!(parse_args(&args(&["patch"])).is_err());
    }

    #[test]
    fn reject_patch_absent_payload() {
        assert!(parse_args(&args(&["patch", "3"])).is_err());
    }

    #[test]
    fn reject_patch_unknown_flag() {
        assert!(parse_args(&args(&["patch", "3", "--merge", "x"])).is_err());
    }

    #[test]
    fn reject_patch_duplicate_flags() {
        assert!(parse_args(&args(&["patch", "3", "--text", "a", "--text", "b"])).is_err());
        assert!(
            parse_args(&args(&[
                "patch",
                "3",
                "--metadata",
                "{}",
                "--metadata",
                "{}"
            ]))
            .is_err()
        );
    }

    #[test]
    fn reject_patch_missing_flag_value() {
        assert!(parse_args(&args(&["patch", "3", "--text"])).is_err());
        assert!(parse_args(&args(&["patch", "3", "--metadata"])).is_err());
    }

    #[test]
    fn reject_patch_extra_positional() {
        assert!(parse_args(&args(&["patch", "3", "extra", "--text", "x"])).is_err());
        assert!(parse_args(&args(&["patch", "3", "--text", "x", "extra"])).is_err());
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
        assert!(parse_args(&args(&["get", "1", "--verbose"])).is_err());
    }

    #[test]
    fn reject_get_without_id() {
        assert!(parse_args(&args(&["get"])).is_err());
    }

    #[test]
    fn reject_extra_argument_get() {
        assert!(parse_args(&args(&["get", "1", "extra"])).is_err());
    }

    #[test]
    fn parse_list_no_args() {
        assert_eq!(
            parse_args(&args(&["list"])),
            Ok(Command::List {
                filter: BTreeMap::new(),
                limit: LIST_DEFAULT_LIMIT,
                offset: 0,
            })
        );
    }

    #[test]
    fn reject_list_format_flag() {
        assert!(parse_args(&args(&["list", "--format", "json"])).is_err());
    }

    #[test]
    fn reject_list_suggest_flag() {
        assert!(parse_args(&args(&["list", "--suggest"])).is_err());
    }

    #[test]
    fn reject_list_unknown_option() {
        assert!(parse_args(&args(&["list", "--verbose"])).is_err());
        assert!(parse_args(&args(&["list", "--details"])).is_err());
    }

    #[test]
    fn reject_list_extra_positional() {
        assert!(parse_args(&args(&["list", "extra"])).is_err());
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
