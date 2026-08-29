//! stdlib-only, gitignore-compatible repository traversal (design D3).
//!
//! Walks the repository root using only the standard library. It applies a
//! self-contained gitignore-compatible matcher plus a built-in skip set, never
//! follows symlinks, and normalizes every emitted path to repository-relative
//! forward-slash form. Directory entries are enumerated in sorted order so the
//! walk is deterministic (design D9).
//!
//! Fail-soft: per-entry I/O/permission errors are recorded as `{reason, path}`
//! limitations and traversal continues (design D8).

use crate::error::LibraryError;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// A visited path with its kind, as emitted by the traversal pipeline.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct WalkEntry {
    /// Absolute path on disk (retained for diagnostics; not read for signals).
    #[allow(dead_code)]
    pub(crate) abs: PathBuf,
    /// Normalized repository-relative forward-slash path.
    pub(crate) rel: String,
    /// Whether the entry is a directory (not a symlink).
    pub(crate) is_dir: bool,
}

/// A per-entry failure recorded as a limitation instead of aborting.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct WalkLimit {
    /// Stable reason string.
    pub(crate) reason: String,
    /// Normalized repository-relative path the limitation concerns (may be "").
    pub(crate) rel: String,
}

/// Result of a full walk: sorted entries plus collected limitations.
#[derive(Debug, Clone, Default)]
pub(crate) struct WalkResult {
    /// All visited, non-skipped entries (sorted by `rel`).
    pub(crate) entries: Vec<WalkEntry>,
    /// Collected fail-soft limitations.
    pub(crate) limits: Vec<WalkLimit>,
}

/// Built-in directory names always skipped, independently of any `.gitignore`.
pub(crate) const BUILTIN_SKIP_DIRS: &[&str] = &[
    ".git",
    ".infraware",
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    ".venv",
    "venv",
    "env",
    "virtualenv",
    "__pycache__",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
];

/// Maximum recursion depth; deeper trees are pruned with a limitation note.
pub(crate) const MAX_DEPTH: usize = 100;

/// Run the full traversal over `root`.
///
/// Returns every non-skipped, non-symlink entry plus collected limitations.
/// Never returns an error for unreadable subtrees; those become limitations.
pub(crate) fn walk(root: &Path) -> Result<WalkResult, LibraryError> {
    if !root.is_absolute() {
        return Err(LibraryError::Io {
            message: format!("root must be absolute: {}", root.display()),
        });
    }

    let mut result = WalkResult::default();
    // Load all .gitignore files from root downward; empty root stack means no
    // ignore rules apply (only built-in skips).
    let ignore = GitignoreMatcher::load(root).unwrap_or_else(|_| GitignoreMatcher::empty());

    collect(root, root, &ignore, 0, &mut result);
    result.entries.sort();
    result.limits.sort();
    Ok(result)
}

/// Recursively collect entries, pruning skipped/subtree-denied directories.
fn collect(
    root: &Path,
    dir: &Path,
    ignore: &GitignoreMatcher,
    depth: usize,
    result: &mut WalkResult,
) {
    if depth > MAX_DEPTH {
        result.limits.push(WalkLimit {
            reason: format!("max depth exceeded at {}", rel_path(root, dir)),
            rel: rel_path(root, dir),
        });
        return;
    }

    let read = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            result.limits.push(WalkLimit {
                reason: format!("unreadable directory {}: {e}", rel_path(root, dir)),
                rel: rel_path(root, dir),
            });
            return;
        }
    };

    // Gather and sort entries by name for deterministic order.
    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in read {
        match entry {
            Ok(e) => entries.push(e.path()),
            Err(e) => {
                result.limits.push(WalkLimit {
                    reason: format!("unreadable directory entry in {}: {e}", rel_path(root, dir)),
                    rel: rel_path(root, dir),
                });
            }
        }
    }
    entries.sort();

    for path in entries {
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => {
                // Non-UTF8 name: skip but record a limitation.
                result.limits.push(WalkLimit {
                    reason: format!("skipping non-utf8 path: {}", path.display()),
                    rel: rel_path(root, &path),
                });
                continue;
            }
        };

        // Symlink detection: never follow symlinks (design D3).
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                result.limits.push(WalkLimit {
                    reason: format!("unreadable path {}: {e}", rel_path(root, &path)),
                    rel: rel_path(root, &path),
                });
                continue;
            }
        };
        if meta.file_type().is_symlink() {
            result.limits.push(WalkLimit {
                reason: format!("symlink skipped (not followed): {}", rel_path(root, &path)),
                rel: rel_path(root, &path),
            });
            continue;
        }

        let is_dir = meta.file_type().is_dir();
        let rel = rel_path(root, &path);

        // Built-in skip applies to directories by name. `.infraware` is the
        // tool's own canonical-storage directory: it is pruned silently so that
        // the presence or absence of the canonical storage on disk cannot alter
        // the analyzer's output (deterministic repository analysis). All other
        // built-in skips are recorded as limitations for transparency.
        if is_dir && BUILTIN_SKIP_DIRS.contains(&file_name) && file_name != ".infraware" {
            // Built-in skip dir that is not the silent `.infraware` prune:
            // record it as a limitation for transparency (see comment above).
            result.limits.push(WalkLimit {
                reason: format!("built-in skip: {rel}"),
                rel: rel.clone(),
            });
        }
        if is_dir && BUILTIN_SKIP_DIRS.contains(&file_name) {
            // Any built-in skip dir (including the silent `.infraware` prune)
            // is pruned from traversal.
            continue;
        }

        // gitignore matching against the repository-relative forward-slash path.
        if ignore.is_ignored(&rel, is_dir) {
            continue;
        }

        // Record the entry.
        result.entries.push(WalkEntry {
            abs: path.clone(),
            rel,
            is_dir,
        });

        // Descend into real directories only.
        if is_dir {
            collect(root, &path, ignore, depth + 1, result);
        }
    }
}

/// Compute the normalized repository-relative forward-slash path.
pub(crate) fn rel_path(root: &Path, path: &Path) -> String {
    let rel = match path.strip_prefix(root) {
        Ok(r) => r,
        Err(_) => path,
    };
    normalize_slashes(rel)
}

/// Convert OS path separators to forward slashes and strip leading separators.
pub(crate) fn normalize_slashes(path: &Path) -> String {
    let mut out = String::new();
    let mut first = true;
    for comp in path.components() {
        use std::path::Component::*;
        match comp {
            Normal(s) => {
                if !first {
                    out.push('/');
                }
                out.push_str(&s.to_string_lossy());
                first = false;
            }
            RootDir => {}
            CurDir => {}
            ParentDir => {
                // Collapse `..` defensively; caller already has a relative path.
                if !first {
                    out.push('/');
                }
                out.push_str("..");
                first = false;
            }
            Prefix(p) => {
                out.push_str(&p.as_os_str().to_string_lossy());
                first = false;
            }
        }
    }
    out
}

/// A self-contained, gitignore-compatible matcher (design D3).
///
/// Supports `*`/`**`/`?` globs, trailing-slash directory-only patterns, and
/// leading-`!` negation. Comment (`#`) and blank lines are ignored. Nested
/// `.gitignore` files apply only to their own subtree.
#[derive(Debug, Clone, Default)]
pub(crate) struct GitignoreMatcher {
    /// Rule groups keyed by the *directory* (relative, forward-slash) they
    /// apply to. The empty key "" covers the repository root.
    pub(crate) rules: BTreeMap<String, Vec<IgnoreRule>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IgnoreRule {
    pub(crate) negated: bool,
    /// Pattern components split on `/` (with `**` markers preserved).
    pub(crate) components: Vec<String>,
    /// Whether the original pattern contained no `/` (matches any depth).
    pub(crate) no_slash: bool,
    /// Whether this pattern is directory-only (trailing slash).
    pub(crate) dir_only: bool,
}

impl GitignoreMatcher {
    /// An empty matcher with no rules (only built-in skips apply).
    pub(crate) fn empty() -> Self {
        Self {
            rules: BTreeMap::new(),
        }
    }

    /// Load every `.gitignore` from `root` downward, building per-directory
    /// rule groups. Fails soft: an unreadable ignore file is skipped.
    pub(crate) fn load(root: &Path) -> Result<Self, LibraryError> {
        let mut rules: BTreeMap<String, Vec<IgnoreRule>> = BTreeMap::new();
        // BFS/DFS for every .gitignore under root.
        let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let gi = dir.join(".gitignore");
            if gi.is_file() {
                let rel = rel_path(root, &dir);
                if let Ok(text) = fs::read_to_string(&gi) {
                    let parsed: Vec<IgnoreRule> = text.lines().filter_map(parse_line).collect();
                    rules.entry(rel).or_default().extend(parsed);
                }
            }
            // Enumerate subdirectories for more .gitignore files.
            if let Ok(rd) = fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    if let Ok(m) = fs::symlink_metadata(&p) {
                        if m.file_type().is_dir()
                            && !BUILTIN_SKIP_DIRS
                                .contains(&p.file_name().and_then(|n| n.to_str()).unwrap_or(""))
                        {
                            stack.push(p);
                        }
                    }
                }
            }
        }
        Ok(Self { rules })
    }

    /// Whether `rel` (repository-relative forward-slash path) is ignored.
    pub(crate) fn is_ignored(&self, rel: &str, is_dir: bool) -> bool {
        // A `.gitignore` in directory `dir` governs `dir` and its subtree; its
        // patterns are interpreted relative to `dir`. So for each rule group we
        // match the pattern against the path as seen from `dir` (the repo-relative
        // `rel` with the `dir/` prefix stripped). Root rules use the full `rel`.
        let mut ignored = false;
        for (dir, rules) in &self.rules {
            let scoped = match dir.as_str() {
                "" => rel.to_string(),
                d => match rel.strip_prefix(&format!("{d}/")) {
                    Some(rest) => rest.to_string(),
                    None => continue,
                },
            };
            for rule in rules {
                // A pattern matches if it matches the scoped (dir-relative) path.
                if rule.matches(&scoped, is_dir) {
                    ignored = !rule.negated;
                }
            }
        }
        ignored
    }
}

/// Parse a single gitignore line into an [`IgnoreRule`], or `None` for comments
/// and blanks.
fn parse_line(line: &str) -> Option<IgnoreRule> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let negated = trimmed.starts_with('!');
    let body = if negated { &trimmed[1..] } else { trimmed };
    let body = body.trim_start();
    // A leading slash anchors to the ignore file's directory only.
    let body = body.strip_prefix('/').unwrap_or(body);
    let dir_only = body.ends_with('/');
    let body = body.strip_suffix('/').unwrap_or(body);

    let (components, no_slash) = compile_glob(body);
    Some(IgnoreRule {
        negated,
        components,
        no_slash,
        dir_only,
    })
}

/// Compile a gitignore pattern body into its matching representation.
///
/// The pattern is split on `/` into components; `**` is kept as a marker.
/// `no_slash` records whether the original pattern contained no `/` at all
/// (so gitignore matches it at any directory level), and `dir_only` records a
/// trailing slash.
fn compile_glob(body: &str) -> (Vec<String>, bool) {
    let no_slash = !body.contains('/');
    let mut comps: Vec<String> = body
        .split('/')
        .filter(|c| !c.is_empty())
        .map(|c| c.to_string())
        .collect();
    if comps.is_empty() {
        // A bare `**` matches everything from the ignore file's directory.
        comps.push("**".to_string());
    }
    (comps, no_slash)
}

impl IgnoreRule {
    /// Match a (dir-relative) `rel` path against this rule.
    pub(crate) fn matches(&self, rel: &str, is_dir: bool) -> bool {
        if self.dir_only && !is_dir {
            return false;
        }
        let name: Vec<&str> = rel.split('/').filter(|c| !c.is_empty()).collect();
        // A pattern with no `/` matches at any directory level beneath the
        // ignore file's location (gitignore semantics). Try every offset.
        if self.no_slash && self.components.iter().all(|c| c.as_str() != "**") {
            for k in 0..=name.len() {
                if pattern_matches(&self.components, &name[k..], self.dir_only) {
                    return true;
                }
            }
            false
        } else {
            pattern_matches(&self.components, &name, self.dir_only)
        }
    }
}

/// Match pattern components `pat` against the remaining path components `name`.
///
/// `**` consumes zero or more path components; directory-only patterns also
/// match any descendant once the pattern is fully consumed.
fn pattern_matches(pat: &[String], name: &[&str], dir_only: bool) -> bool {
    if pat.is_empty() {
        // Exact consumption: a match. For dir-only patterns, also match any
        // path at or beneath the directory (remaining components present).
        return name.is_empty() || dir_only;
    }
    if pat[0] == "**" {
        // `**` matches zero or more components: try every skip amount.
        for k in 0..=name.len() {
            if pattern_matches(&pat[1..], &name[k..], dir_only) {
                return true;
            }
        }
        return false;
    }
    if name.is_empty() {
        return false;
    }
    if glob_component(&pat[0], name[0]) && pattern_matches(&pat[1..], &name[1..], dir_only) {
        return true;
    }
    false
}

/// Match one pattern component (which may contain `*`/`?`) against one path
/// component (a single filename). `*` matches any run of chars (no `/`), `?`
/// matches exactly one char.
fn glob_component(pat: &str, name: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let n: Vec<char> = name.chars().collect();
    fn rec(p: &[char], n: &[char]) -> bool {
        if p.is_empty() {
            return n.is_empty();
        }
        match p[0] {
            '*' => {
                // `*` matches a prefix (possibly empty) of the remaining name.
                for k in 0..=n.len() {
                    if rec(&p[1..], &n[k..]) {
                        return true;
                    }
                }
                false
            }
            '?' => {
                if n.is_empty() {
                    false
                } else {
                    rec(&p[1..], &n[1..])
                }
            }
            c => {
                if n.first().copied() == Some(c) {
                    rec(&p[1..], &n[1..])
                } else {
                    false
                }
            }
        }
    }
    rec(&p, &n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negation_overrides_earlier_rule() {
        let text = "*.log\n!important.log\n";
        let mut rules: BTreeMap<String, Vec<IgnoreRule>> = BTreeMap::new();
        rules.insert(String::new(), text.lines().filter_map(parse_line).collect());
        let m = GitignoreMatcher { rules };
        assert!(m.is_ignored("debug.log", false));
        assert!(!m.is_ignored("important.log", false));
    }

    #[test]
    fn directory_only_pattern() {
        let text = "build/\n";
        let mut rules: BTreeMap<String, Vec<IgnoreRule>> = BTreeMap::new();
        rules.insert(String::new(), text.lines().filter_map(parse_line).collect());
        let m = GitignoreMatcher { rules };
        assert!(m.is_ignored("build", true));
        assert!(m.is_ignored("build/foo", true));
        assert!(!m.is_ignored("build", false));
        assert!(!m.is_ignored("build.txt", false));
    }

    #[test]
    fn star_glob_matches_basename() {
        let text = "*.tmp\n";
        let mut rules: BTreeMap<String, Vec<IgnoreRule>> = BTreeMap::new();
        rules.insert(String::new(), text.lines().filter_map(parse_line).collect());
        let m = GitignoreMatcher { rules };
        assert!(m.is_ignored("a/b.tmp", false));
        assert!(!m.is_ignored("a/b.txt", false));
    }

    #[test]
    fn nested_ignore_scopes_to_subtree() {
        let mut rules: BTreeMap<String, Vec<IgnoreRule>> = BTreeMap::new();
        rules.insert(String::new(), vec![parse_line("root-only").unwrap()]);
        rules.insert("sub".to_string(), vec![parse_line("nested").unwrap()]);
        let m = GitignoreMatcher { rules };
        assert!(m.is_ignored("root-only", false));
        assert!(!m.is_ignored("sub/other", false));
        assert!(m.is_ignored("sub/nested", false));
        assert!(!m.is_ignored("nested", false));
    }

    #[test]
    fn normalize_slashes_is_forward() {
        let p = Path::new("a//b/./c");
        let n = normalize_slashes(p);
        assert!(!n.contains('\\'));
        assert!(!n.contains("./"));
    }

    #[test]
    fn builtin_skip_set_contains_expected() {
        assert!(BUILTIN_SKIP_DIRS.contains(&"target"));
        assert!(BUILTIN_SKIP_DIRS.contains(&".git"));
    }
}
