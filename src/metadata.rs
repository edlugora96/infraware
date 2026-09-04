//! Ingest metadata validation, canonicalization, and identity hashing.
//!
//! Tasks 2.1/2.2 only. No storage, idempotency, embedding, or dispatch changes.

use std::collections::BTreeMap;

use crate::error::LibraryError;
use crate::identity::validate_id;

/// Maximum ingest text size in bytes (64 KiB).
pub(crate) const MAX_TEXT_BYTES: usize = 64 * 1024;

/// Validate ingest text: non-empty/non-whitespace, then byte length limit.
pub(crate) fn validate_text(text: &str) -> Result<(), LibraryError> {
    if text.trim().is_empty() {
        return Err(LibraryError::InvalidInput {
            message: "ingest text must not be empty".to_string(),
        });
    }
    if text.len() > MAX_TEXT_BYTES {
        return Err(LibraryError::InvalidInput {
            message: format!(
                "ingest text exceeds {} bytes (got {})",
                MAX_TEXT_BYTES,
                text.len()
            ),
        });
    }
    Ok(())
}

/// Canonicalize optional raw metadata JSON.
///
/// - `None` → `"{}"`.
/// - Malformed JSON → `InvalidInput`.
/// - Only a flat string-valued object is accepted.
/// - Duplicate top-level keys (escape-aware, post-unescape) → `InvalidInput`.
/// - Keys validated via [`validate_id`], mapped to `InvalidInput`.
/// - Returns compact `serde_json` rendering with `BTreeMap` ordering.
pub(crate) fn canonicalize_metadata(raw: Option<&str>) -> Result<String, LibraryError> {
    let Some(s) = raw else {
        return Ok("{}".to_string());
    };
    let value: serde_json::Value =
        serde_json::from_str(s).map_err(|e| LibraryError::InvalidInput {
            message: format!("malformed metadata JSON: {e}"),
        })?;
    let obj = match value.as_object() {
        Some(o) => o,
        None => {
            return Err(LibraryError::InvalidInput {
                message: "metadata must be a JSON object".to_string(),
            });
        }
    };
    let mut flat: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in obj {
        match v.as_str() {
            Some(s) => {
                flat.insert(k.clone(), s.to_string());
            }
            None => {
                return Err(LibraryError::InvalidInput {
                    message: format!("metadata value for key {k:?} must be a string"),
                });
            }
        }
    }
    detect_duplicate_keys(s)?;
    for k in flat.keys() {
        validate_id(k).map_err(|e| LibraryError::InvalidInput {
            message: e.to_string(),
        })?;
    }
    serde_json::to_string(&flat).map_err(|e| LibraryError::InvalidInput {
        message: format!("failed to canonicalize metadata: {e}"),
    })
}

/// Compute identity: lowercase hex SHA-256(canonical bytes + 0x0A + exact text bytes).
pub(crate) fn compute_identity(canonical_metadata: &str, text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(canonical_metadata.as_bytes());
    h.update([0x0Au8]);
    h.update(text.as_bytes());
    let bytes = h.finalize();
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    s
}

/// Escape-aware raw scan for duplicate top-level object keys.
///
/// Operates on the raw JSON text (already known to parse). Extracts top-level
/// keys, unescapes them, and reports duplicates including escaped-equivalent
/// forms (e.g. `"a"` vs `"\u0061"`).
fn detect_duplicate_keys(raw: &str) -> Result<(), LibraryError> {
    let bytes = raw.as_bytes();
    let mut pos = skip_ws(bytes, 0);
    // Must start with '{' since caller verified it's an object.
    if pos >= bytes.len() || bytes[pos] != b'{' {
        return Ok(());
    }
    pos += 1;
    let mut keys: Vec<String> = Vec::new();
    pos = skip_ws(bytes, pos);
    if pos < bytes.len() && bytes[pos] == b'}' {
        return Ok(());
    }
    loop {
        pos = skip_ws(bytes, pos);
        if pos >= bytes.len() || bytes[pos] != b'"' {
            return Ok(());
        }
        let (key, next) =
            parse_raw_string(bytes, pos).map_err(|m| LibraryError::InvalidInput { message: m })?;
        if keys.contains(&key) {
            return Err(LibraryError::InvalidInput {
                message: format!("duplicate metadata key {key:?}"),
            });
        }
        keys.push(key);
        pos = skip_ws(bytes, next);
        if pos >= bytes.len() || bytes[pos] != b':' {
            return Ok(());
        }
        pos += 1;
        pos = skip_json_value(bytes, pos).map_err(|m| LibraryError::InvalidInput { message: m })?;
        pos = skip_ws(bytes, pos);
        if pos >= bytes.len() {
            return Ok(());
        }
        if bytes[pos] == b',' {
            pos += 1;
            continue;
        }
        return Ok(());
    }
}

fn skip_ws(bytes: &[u8], mut pos: usize) -> usize {
    while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r') {
        pos += 1;
    }
    pos
}

/// Parse a JSON string starting at `pos` (which must be `"`), returning the
/// unescaped key and the position one past the closing quote.
fn parse_raw_string(bytes: &[u8], pos: usize) -> Result<(String, usize), String> {
    let mut out = String::new();
    let mut i = pos + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => return Ok((out, i + 1)),
            b'\\' => {
                i += 1;
                if i >= bytes.len() {
                    return Err("truncated escape in metadata key".to_string());
                }
                match bytes[i] {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\x08'),
                    b'f' => out.push('\x0C'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        if i + 4 >= bytes.len() {
                            return Err("truncated \\u escape in metadata key".to_string());
                        }
                        let hex = std::str::from_utf8(&bytes[i + 1..i + 5])
                            .map_err(|_| "invalid \\u escape".to_string())?;
                        let cp = u32::from_str_radix(hex, 16)
                            .map_err(|_| "invalid \\u escape".to_string())?;
                        i += 4;
                        // Handle surrogate pairs.
                        if (0xD800..0xDC00).contains(&cp) {
                            if i + 5 < bytes.len() && bytes[i + 1] == b'\\' && bytes[i + 2] == b'u'
                            {
                                let hex2 = std::str::from_utf8(&bytes[i + 3..i + 7])
                                    .map_err(|_| "invalid low surrogate".to_string())?;
                                let lo = u32::from_str_radix(hex2, 16)
                                    .map_err(|_| "invalid low surrogate".to_string())?;
                                if (0xDC00..0xE000).contains(&lo) {
                                    let c = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                    out.push(
                                        char::from_u32(c)
                                            .ok_or_else(|| "invalid codepoint".to_string())?,
                                    );
                                    i += 6;
                                } else {
                                    return Err("invalid low surrogate".to_string());
                                }
                            } else {
                                return Err("lone surrogate in metadata key".to_string());
                            }
                        } else {
                            out.push(
                                char::from_u32(cp)
                                    .ok_or_else(|| "invalid codepoint".to_string())?,
                            );
                        }
                    }
                    other => {
                        return Err(format!("invalid escape '\\{other}' in metadata key"));
                    }
                }
                i += 1;
            }
            _ => {
                // Regular byte; keys are expected to be valid UTF-8 slices.
                // Consume one UTF-8 char via str fallback.
                let s = std::str::from_utf8(&bytes[i..])
                    .map_err(|_| "invalid utf-8 in metadata key".to_string())?;
                let ch = s
                    .chars()
                    .next()
                    .ok_or("truncated metadata key".to_string())?;
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    Err("unterminated string in metadata".to_string())
}

/// Skip one JSON value starting at `pos`; returns position past the value.
fn skip_json_value(bytes: &[u8], pos: usize) -> Result<usize, String> {
    let mut p = skip_ws(bytes, pos);
    if p >= bytes.len() {
        return Err("truncated JSON value".to_string());
    }
    match bytes[p] {
        b'"' => {
            let (_, next) = parse_raw_string(bytes, p)?;
            Ok(next)
        }
        b'{' => {
            p += 1;
            p = skip_ws(bytes, p);
            if p < bytes.len() && bytes[p] == b'}' {
                return Ok(p + 1);
            }
            loop {
                p = skip_ws(bytes, p);
                if p >= bytes.len() || bytes[p] != b'"' {
                    return Err("expected string key".to_string());
                }
                let (_, next) = parse_raw_string(bytes, p)?;
                p = skip_ws(bytes, next);
                if p >= bytes.len() || bytes[p] != b':' {
                    return Err("expected ':'".to_string());
                }
                p = skip_json_value(bytes, p + 1)?;
                p = skip_ws(bytes, p);
                if p >= bytes.len() {
                    return Err("truncated object".to_string());
                }
                if bytes[p] == b',' {
                    p += 1;
                    continue;
                }
                if bytes[p] == b'}' {
                    return Ok(p + 1);
                }
                return Err("expected ',' or '}'".to_string());
            }
        }
        b'[' => {
            p += 1;
            p = skip_ws(bytes, p);
            if p < bytes.len() && bytes[p] == b']' {
                return Ok(p + 1);
            }
            loop {
                p = skip_json_value(bytes, p)?;
                p = skip_ws(bytes, p);
                if p >= bytes.len() {
                    return Err("truncated array".to_string());
                }
                if bytes[p] == b',' {
                    p += 1;
                    continue;
                }
                if bytes[p] == b']' {
                    return Ok(p + 1);
                }
                return Err("expected ',' or ']'".to_string());
            }
        }
        b't' => {
            expect_lit(bytes, p, "true")?;
            Ok(p + 4)
        }
        b'f' => {
            expect_lit(bytes, p, "false")?;
            Ok(p + 5)
        }
        b'n' => {
            expect_lit(bytes, p, "null")?;
            Ok(p + 4)
        }
        b'-' | b'0'..=b'9' => {
            p += 1;
            while p < bytes.len()
                && matches!(bytes[p], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
            {
                p += 1;
            }
            Ok(p)
        }
        _ => Err("unexpected JSON value".to_string()),
    }
}

fn expect_lit(bytes: &[u8], pos: usize, lit: &str) -> Result<(), String> {
    if bytes[pos..].starts_with(lit.as_bytes()) {
        Ok(())
    } else {
        Err(format!("invalid literal near {lit}"))
    }
}

/// Convenience: validate text, canonicalize metadata, and hash identity.
pub(crate) fn prepare(
    text: &str,
    metadata: Option<&str>,
) -> Result<(String, String), LibraryError> {
    validate_text(text)?;
    let canonical = canonicalize_metadata(metadata)?;
    let id = compute_identity(&canonical, text);
    Ok((canonical, id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_text() {
        assert!(validate_text("").is_err());
    }

    #[test]
    fn rejects_whitespace_text() {
        assert!(validate_text("   \n\t ").is_err());
    }

    #[test]
    fn rejects_overlong_text() {
        let s = "a".repeat(MAX_TEXT_BYTES + 1);
        assert!(validate_text(&s).is_err());
    }

    #[test]
    fn accepts_boundary_length() {
        let s = "a".repeat(MAX_TEXT_BYTES);
        assert!(validate_text(&s).is_ok());
    }

    #[test]
    fn absent_metadata_is_empty_object() {
        assert_eq!(canonicalize_metadata(None).unwrap(), "{}");
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(canonicalize_metadata(Some("{oops")).is_err());
    }

    #[test]
    fn rejects_non_object() {
        assert!(canonicalize_metadata(Some("[1,2]")).is_err());
        assert!(canonicalize_metadata(Some("\"x\"")).is_err());
    }

    #[test]
    fn rejects_nested_object_value() {
        assert!(canonicalize_metadata(Some(r#"{"a":{"b":"c"}}"#)).is_err());
    }

    #[test]
    fn rejects_non_string_value() {
        assert!(canonicalize_metadata(Some(r#"{"a":1}"#)).is_err());
        assert!(canonicalize_metadata(Some(r#"{"a":null}"#)).is_err());
        assert!(canonicalize_metadata(Some(r#"{"a":["x"]}"#)).is_err());
    }

    #[test]
    fn rejects_duplicate_keys() {
        assert!(canonicalize_metadata(Some(r#"{"a":"1","a":"2"}"#)).is_err());
    }

    #[test]
    fn rejects_escaped_equivalent_duplicate_keys() {
        assert!(canonicalize_metadata(Some(r#"{"a":"1","\u0061":"2"}"#)).is_err());
    }

    #[test]
    fn rejects_invalid_id_key() {
        assert!(canonicalize_metadata(Some(r#"{"a b":"1"}"#)).is_err());
        assert!(canonicalize_metadata(Some(r#"{"":"1"}"#)).is_err());
    }

    #[test]
    fn orders_keys_and_compact_canonical() {
        let c = canonicalize_metadata(Some(r#"{"z":"1","a":"2"}"#)).unwrap();
        assert_eq!(c, r#"{"a":"2","z":"1"}"#);
    }

    #[test]
    fn canonical_golden_vector() {
        // Whitespace/ordering normalized to compact BTreeMap form.
        let c = canonicalize_metadata(Some("{ \"b\" : \"2\" , \"a\" : \"1\" }")).unwrap();
        assert_eq!(c, r#"{"a":"1","b":"2"}"#);
    }

    #[test]
    fn identity_golden_vector() {
        // SHA-256("{}" + 0x0A + "hello") lowercase hex.
        // Precomputed golden vector.
        let expected = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(b"{}");
            h.update([0x0Au8]);
            h.update(b"hello");
            let bytes = h.finalize();
            let mut s = String::new();
            for b in bytes {
                s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
                s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
            }
            s
        };
        assert_eq!(compute_identity("{}", "hello"), expected);
        assert_eq!(compute_identity("{}", "hello").len(), 64);
    }

    #[test]
    fn identity_differs_by_text_and_metadata() {
        assert_ne!(compute_identity("{}", "a"), compute_identity("{}", "b"));
        assert_ne!(
            compute_identity("{}", "a"),
            compute_identity(r#"{"k":"v"}"#, "a")
        );
    }
}
