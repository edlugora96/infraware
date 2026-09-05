//! Stdlib-only exact-argument parser.
//!
//! Accepts exactly the approved eight-verb grammar and nothing else. No `clap`
//! or any external crate is used (design D7). Removed legacy verbs (`init`,
//! `analyze`, `insert`, `edit`), aliases (`set`, `rm`), and removed legacy
//! flags (`--format`, `--suggest`) are rejected as usage errors.

use crate::command::{Command, Entity, Invocation, InvocationOptions};
use crate::error::LibraryError;
use crate::format::OutputFormat;

/// Parse an argv slice (excluding the program name) into a typed [`Command`].
///
/// Returns [`LibraryError::InvalidInput`] for any grammar violation: unknown
/// command, unknown option, or malformed shape. Only the eight SQLite-backed
/// verbs (`ingest`, `query`, `list`, `get`, `patch`, `remove`, `status`, `revise`) are accepted;
/// legacy verbs (`init`, `analyze`, `insert`, `edit`), aliases (`set`, `rm`),
/// and removed legacy flags (`--format`, `--suggest`) are rejected.
pub fn parse_args(argv: &[String]) -> Result<Invocation, LibraryError> {
    let mut args = argv.to_vec();
    let mut detailed = false;
    if args.last().is_some_and(|x| x == "--detailed") {
        detailed = true;
        args.pop();
    }
    let mut format = OutputFormat::Yaml;
    if args.len() >= 2 && args[args.len() - 2] == "--format" {
        format = OutputFormat::parse(&args[args.len() - 1]).ok_or_else(|| {
            LibraryError::InvalidInput {
                message: "invalid --format value".into(),
            }
        })?;
        args.truncate(args.len() - 2);
    } else if args.iter().any(|x| x == "--format" || x == "--detailed") {
        return Err(LibraryError::InvalidInput {
            message: "shared options must be trailing".into(),
        });
    }
    let command = parse_command(&args)?;
    Ok(Invocation {
        command,
        options: InvocationOptions { format, detailed },
    })
}

fn parse_command(argv: &[String]) -> Result<Command, LibraryError> {
    let mut iter = argv.iter();

    let Some(command_token) = iter.next() else {
        return Err(LibraryError::InvalidInput {
            message: "no command provided".to_string(),
        });
    };

    match command_token.as_str() {
        "status" => {
            if iter.next().is_some() {
                Err(LibraryError::InvalidInput {
                    message: "status takes no arguments".into(),
                })
            } else {
                Ok(Command::Status)
            }
        }
        "ingest" => parse_ingest(&mut iter),
        "query" => parse_query(&mut iter),
        "list" => parse_list(&mut iter),
        "get" => parse_get(&mut iter),
        "patch" => parse_patch(&mut iter),
        "remove" => parse_remove(&mut iter),
        "revise" => parse_revise(&mut iter),
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
    if id.starts_with("--") {
        return Err(LibraryError::InvalidInput {
            message: "get requires an <ID>".into(),
        });
    }
    let entity = parse_entity_option(iter, false, "get")?;
    Ok(Command::Get {
        id: id.clone(),
        entity,
    })
}

fn parse_entity_value(raw: &str, command: &str) -> Result<Entity, LibraryError> {
    match raw {
        "rules" => Ok(Entity::Rules),
        "tasks" => Ok(Entity::Tasks),
        _ => Err(LibraryError::InvalidInput {
            message: format!("invalid --entity value for {command}: {raw}"),
        }),
    }
}

fn parse_entity_option<'a, I>(
    iter: &mut I,
    allow_all: bool,
    command: &str,
) -> Result<Entity, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    let Some(flag) = iter.next() else {
        return Ok(Entity::Rules);
    };
    if flag != "--entity" {
        return Err(LibraryError::InvalidInput {
            message: format!("unexpected extra argument for {command}: {flag}"),
        });
    }
    let value = iter.next().ok_or_else(|| LibraryError::InvalidInput {
        message: "--entity requires a value".into(),
    })?;
    if !allow_all && value == "all" {
        return Err(LibraryError::InvalidInput {
            message: format!("invalid --entity value for {command}: all"),
        });
    }
    let entity = parse_entity_value(value, command)?;
    if iter.next().is_some() {
        return Err(LibraryError::InvalidInput {
            message: format!("duplicate --entity option for {command}"),
        });
    }
    Ok(entity)
}

/// Default page size for `list` when no limit positional is given.
pub(crate) const LIST_DEFAULT_LIMIT: usize = 50;
/// Maximum page size for `list`; larger limits clamp to this value.
pub(crate) const LIST_MAX_LIMIT: usize = 500;

/// Parse a list limit/offset positional: ASCII digits only, fits `u64`.
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

/// Parse `list [LIMIT] [OFFSET]` with no flags.
fn parse_list<'a, I>(iter: &mut I) -> Result<Command, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    let positional: Vec<&String> = iter.collect();
    if positional.len() > 2 {
        return Err(LibraryError::InvalidInput {
            message: "unexpected extra argument for list".into(),
        });
    }
    let limit = positional
        .first()
        .map(|s| parse_list_count(s, "limit"))
        .transpose()?
        .unwrap_or(LIST_DEFAULT_LIMIT)
        .min(LIST_MAX_LIMIT);
    let offset = positional
        .get(1)
        .map(|s| parse_list_count(s, "offset"))
        .transpose()?
        .unwrap_or(0);
    Ok(Command::List { limit, offset })
}

/// Parse `ingest <TASK_TEXT> <RULE_TEXT>` with exactly two positionals.
fn parse_ingest<'a, I>(iter: &mut I) -> Result<Command, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    let task_text = iter.next().ok_or_else(|| LibraryError::InvalidInput {
        message: "ingest requires <TASK_TEXT> <RULE_TEXT>".to_string(),
    })?;
    let rule_text = iter.next().ok_or_else(|| LibraryError::InvalidInput {
        message: "ingest requires <TASK_TEXT> <RULE_TEXT>".to_string(),
    })?;
    if task_text.starts_with("--") || rule_text.starts_with("--") {
        return Err(LibraryError::InvalidInput {
            message: "ingest text cannot be an option".to_string(),
        });
    }
    if task_text.trim().is_empty() || rule_text.trim().is_empty() {
        return Err(LibraryError::InvalidInput {
            message: "ingest text must not be empty".to_string(),
        });
    }
    if let Some(extra) = iter.next() {
        return Err(LibraryError::InvalidInput {
            message: format!("unexpected extra argument for ingest: {extra}"),
        });
    }
    Ok(Command::Ingest {
        task_text: task_text.clone(),
        rule_text: rule_text.clone(),
    })
}

/// Parse `query <TASK_TEXT>` with exactly one positional argument and no flags.
/// Text must be non-blank and is otherwise preserved byte-exactly.
fn parse_query<'a, I>(iter: &mut I) -> Result<Command, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    let text = iter.next().ok_or_else(|| LibraryError::InvalidInput {
        message: "query requires <TASK_TEXT>".to_string(),
    })?;
    if text.starts_with("--") || text.trim().is_empty() {
        return Err(LibraryError::InvalidInput {
            message: "query requires non-empty <TASK_TEXT>".to_string(),
        });
    }
    if let Some(extra) = iter.next() {
        return Err(LibraryError::InvalidInput {
            message: format!("unexpected extra argument for query: {extra}"),
        });
    }
    Ok(Command::Query { text: text.clone() })
}

/// Parse `patch <ID> --text <TEXT>`.
///
/// `<ID>` is required and carried raw; text validation happens at execution.
/// Unknown flags, duplicate flags, missing values, and absent text are usage errors.
fn parse_patch<'a, I>(iter: &mut I) -> Result<Command, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    let id = iter.next().ok_or_else(|| LibraryError::InvalidInput {
        message: "patch requires an <ID>".to_string(),
    })?;
    if id.starts_with("--") {
        return Err(LibraryError::InvalidInput {
            message: "patch requires an <ID>".into(),
        });
    }
    let mut entity = Entity::Rules;
    let mut entity_seen = false;
    let mut text: Option<String> = None;
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
                if value.trim().is_empty() {
                    return Err(LibraryError::InvalidInput {
                        message: "--text requires non-empty text".to_string(),
                    });
                }
                text = Some(value.clone());
            }
            "--entity" => {
                if entity_seen {
                    return Err(LibraryError::InvalidInput {
                        message: "duplicate --entity option for patch".into(),
                    });
                }
                entity_seen = true;
                let value = iter.next().ok_or_else(|| LibraryError::InvalidInput {
                    message: "--entity requires a value".into(),
                })?;
                entity = parse_entity_value(value, "patch")?;
            }
            "--metadata" => {
                return Err(LibraryError::InvalidInput {
                    message: "unknown option for patch: --metadata".to_string(),
                });
            }
            other => {
                return Err(LibraryError::InvalidInput {
                    message: format!("unknown option for patch: {other}"),
                });
            }
        }
    }
    if text.is_none() {
        return Err(LibraryError::InvalidInput {
            message: "patch requires --text".to_string(),
        });
    }
    Ok(Command::Patch {
        id: id.clone(),
        text,
        entity,
    })
}

/// Parse `revise <ID> --entity rules|tasks`.
fn parse_revise<'a, I>(iter: &mut I) -> Result<Command, LibraryError>
where
    I: Iterator<Item = &'a String>,
{
    let id = iter.next().ok_or_else(|| LibraryError::InvalidInput {
        message: "revise requires an <ID>".into(),
    })?;
    if id.starts_with("--") {
        return Err(LibraryError::InvalidInput {
            message: "revise requires an <ID>".into(),
        });
    }
    let flag = iter.next().ok_or_else(|| LibraryError::InvalidInput {
        message: "revise requires --entity".into(),
    })?;
    if flag != "--entity" {
        return Err(LibraryError::InvalidInput {
            message: format!("unknown option for revise: {flag}"),
        });
    }
    let value = iter.next().ok_or_else(|| LibraryError::InvalidInput {
        message: "--entity requires a value".into(),
    })?;
    let entity = parse_entity_value(value, "revise")?;
    if iter.next().is_some() {
        return Err(LibraryError::InvalidInput {
            message: "unexpected extra argument for revise".into(),
        });
    }
    Ok(Command::Revise {
        id: id.clone(),
        entity,
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
