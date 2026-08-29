//! Evidence classifiers (design D4/D5).
//!
//! Every classifier inspects only the normalized relative path, filename, and
//! extension of a walked entry — never file contents. This keeps the analyzer
//! O(paths)-bounded, read-only, and safe for secret-suspect files (design D5).

use crate::analyze::traversal::WalkEntry;
use std::collections::BTreeSet;

/// The set of signal groups observed during a scan. Used for `--suggest`
/// selection (design D7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SignalGroups {
    /// Observed language names.
    pub(crate) languages: BTreeSet<String>,
    /// Observed manifest kinds.
    pub(crate) manifests: BTreeSet<String>,
    /// Observed linter names.
    pub(crate) linters: BTreeSet<String>,
    /// Observed test group names.
    pub(crate) tests: BTreeSet<String>,
    /// Observed container kinds.
    pub(crate) containers: BTreeSet<String>,
    /// Observed CI/CD system names.
    pub(crate) cicd: BTreeSet<String>,
    /// Observed entry-point kinds.
    pub(crate) entry_points: BTreeSet<String>,
    /// Whether any sensitive file was detected.
    pub(crate) sensitive: bool,
}

/// A single typed classification for one entry.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Signal {
    Language { name: String, path: String },
    Manifest { kind: String, path: String },
    Linter { name: String, path: String },
    Test { name: String, path: String },
    Container { kind: String, path: String },
    Cicd { name: String, path: String },
    EntryPoint { kind: String, path: String },
    Sensitive { path: String, signal: String },
}

/// Classify one walked entry into zero or more signals (path/name only).
pub(crate) fn classify(entry: &WalkEntry) -> Vec<Signal> {
    let rel = &entry.rel;
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let lower = name.to_lowercase();
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let mut out: Vec<Signal> = Vec::new();

    // --- Sensitive (check first; secret protection is path-only) ---
    if let Some(sig) = secret_signal(rel, name, &lower, &ext) {
        out.push(Signal::Sensitive {
            path: rel.clone(),
            signal: sig,
        });
    }

    // --- Manifest ---
    if let Some(kind) = manifest_kind(name, &lower, &ext) {
        out.push(Signal::Manifest {
            kind,
            path: rel.clone(),
        });
    }

    // --- Language (extension-based) ---
    if let Some(lang) = language_kind(&ext) {
        out.push(Signal::Language {
            name: lang,
            path: rel.clone(),
        });
    }

    // --- Linter config ---
    if let Some(name) = linter_kind(rel, name, &lower, &ext) {
        out.push(Signal::Linter {
            name,
            path: rel.clone(),
        });
    }

    // --- Test ---
    if let Some(name) = test_kind(rel, name, &lower, &ext) {
        out.push(Signal::Test {
            name,
            path: rel.clone(),
        });
    }

    // --- Container ---
    if let Some(kind) = container_kind(rel, name, &lower, &ext) {
        out.push(Signal::Container {
            kind,
            path: rel.clone(),
        });
    }

    // --- CI/CD ---
    if let Some(name) = cicd_kind(rel, name, &lower, &ext) {
        out.push(Signal::Cicd {
            name,
            path: rel.clone(),
        });
    }

    // --- Entry point ---
    if let Some(kind) = entry_point_kind(name, &lower, &ext) {
        out.push(Signal::EntryPoint {
            kind,
            path: rel.clone(),
        });
    }

    out
}

/// Conservative secret-suspect classification by name/extension only (D5).
fn secret_signal(rel: &str, name: &str, lower: &str, ext: &str) -> Option<String> {
    // Exact names / patterns.
    if lower == ".env" || lower.starts_with(".env.") || lower.ends_with(".env") {
        return Some("env-file".to_string());
    }
    if lower == "credentials" || lower == "credentials.json" {
        return Some("credentials-file".to_string());
    }
    if lower == "secret"
        || lower == "secrets"
        || lower.starts_with("secret.")
        || lower.ends_with(".secret")
    {
        return Some("secret-file".to_string());
    }
    if lower.contains("token") {
        return Some("token-file".to_string());
    }
    // Extension-based secret suspects.
    match ext {
        "pem" => return Some("pem-key".to_string()),
        "key" => return Some("private-key".to_string()),
        "p12" => return Some("pkcs12-key".to_string()),
        "pfx" => return Some("pkcs12-key".to_string()),
        _ => {}
    }
    if lower == "id_rsa" || lower == "id_dsa" || lower == "id_ecdsa" || lower == "id_ed25519" {
        return Some("ssh-private-key".to_string());
    }
    // Known secret-bearing config locations.
    if rel.contains("/.env.") || rel.ends_with("/.env") {
        return Some("env-file".to_string());
    }
    let _ = name;
    None
}

/// Map a manifest filename to its kind.
fn manifest_kind(name: &str, lower: &str, ext: &str) -> Option<String> {
    let _ = name;
    match lower {
        "cargo.toml" => Some("cargo".to_string()),
        "package.json" => Some("npm".to_string()),
        "package-lock.json" => Some("npm".to_string()),
        "go.mod" => Some("go".to_string()),
        "go.sum" => Some("go".to_string()),
        "pyproject.toml" => Some("python".to_string()),
        "requirements.txt" => Some("python".to_string()),
        "setup.py" => Some("python".to_string()),
        "pom.xml" => Some("maven".to_string()),
        "build.gradle" => Some("gradle".to_string()),
        "gemfile" => Some("ruby".to_string()),
        _ => {
            if ext == "csproj" || ext == "sln" {
                Some("dotnet".to_string())
            } else if ext == "toml" && lower.contains("pyproject") {
                Some("python".to_string())
            } else {
                None
            }
        }
    }
}

/// Map a file extension to a language name.
fn language_kind(ext: &str) -> Option<String> {
    let lang = match ext {
        "rs" => "Rust",
        "js" | "mjs" | "cjs" | "jsx" | "tsx" => "JavaScript",
        "ts" => "TypeScript",
        "py" => "Python",
        "go" => "Go",
        "java" => "Java",
        "rb" => "Ruby",
        "c" | "h" => "C",
        "cpp" | "cc" | "cxx" | "hpp" => "C++",
        "cs" => "C#",
        "php" => "PHP",
        "sh" | "bash" => "Shell",
        "kt" | "kts" => "Kotlin",
        "swift" => "Swift",
        _ => return None,
    };
    Some(lang.to_string())
}

/// Map a linter/config file to a tool name.
fn linter_kind(rel: &str, name: &str, lower: &str, ext: &str) -> Option<String> {
    match lower {
        ".eslintrc" | ".eslintrc.js" | ".eslintrc.json" | ".eslintrc.yml" => {
            Some("eslint".to_string())
        }
        ".prettierrc" | "prettier.config.js" => Some("prettier".to_string()),
        ".flake8" | ".pylintrc" | "pyproject.toml" => {
            // pyproject already counts as a manifest; only count as linter if
            // not the manifest handler. Keep as flake8/pylint proxy.
            if lower == "pyproject.toml" {
                None
            } else {
                Some("python-linter".to_string())
            }
        }
        "clippy.toml" => Some("clippy".to_string()),
        "rustfmt.toml" => Some("rustfmt".to_string()),
        ".rustfmt.toml" => Some("rustfmt".to_string()),
        _ => {
            if lower.ends_with(".editorconfig") {
                Some("editorconfig".to_string())
            } else if rel.contains("/.github/") && lower.contains("linters") {
                Some("github-actions-lint".to_string())
            } else {
                let _ = (name, ext);
                None
            }
        }
    }
}

/// Map a file/dir to a test signal.
fn test_kind(rel: &str, name: &str, lower: &str, ext: &str) -> Option<String> {
    // Nested test directories (e.g. `src/foo/test/...`).
    if rel.contains("/test/")
        || rel.contains("/tests/")
        || rel.contains("/spec/")
        || rel.contains("/__tests__/")
    {
        return Some("test-dir".to_string());
    }
    // Root-level test directories (e.g. `test/...`, `tests/...`, `spec/...`,
    // `__tests__/...`). These are prefixes of the repository-relative path and
    // must be detected independently of any parent component.
    if rel == "test"
        || rel == "tests"
        || rel == "spec"
        || rel == "__tests__"
        || rel.starts_with("test/")
        || rel.starts_with("tests/")
        || rel.starts_with("spec/")
        || rel.starts_with("__tests__/")
    {
        return Some("test-dir".to_string());
    }
    if lower.starts_with("test_") || lower.ends_with("_test.rs") || lower.ends_with(".test.ts") {
        return Some("test-file".to_string());
    }
    if lower == "cargo.toml" && rel.contains("/tests/") {
        return Some("cargo-test".to_string());
    }
    if ext == "rs" && name.contains("test") {
        return Some("cargo-test".to_string());
    }
    if ext == "py" && (lower.starts_with("test_") || lower.ends_with("_test.py")) {
        return Some("pytest".to_string());
    }
    if (ext == "js" || ext == "ts")
        && (lower.ends_with(".spec.ts")
            || lower.ends_with(".spec.js")
            || lower.ends_with(".test.ts")
            || lower.ends_with(".test.js"))
    {
        return Some("jest".to_string());
    }
    let _ = name;
    None
}

/// Map a file to a container signal.
fn container_kind(rel: &str, name: &str, lower: &str, ext: &str) -> Option<String> {
    if lower == "dockerfile" || lower == "dockerfile.dev" || lower == "dockerfile.prod" {
        return Some("dockerfile".to_string());
    }
    if lower == "docker-compose.yml" || lower == "docker-compose.yaml" {
        return Some("docker-compose".to_string());
    }
    if lower == "compose.yml" || lower == "compose.yaml" {
        return Some("docker-compose".to_string());
    }
    if lower == "containerfile" {
        return Some("dockerfile".to_string());
    }
    let _ = (name, rel, ext);
    None
}

/// Map a file to a CI/CD signal.
fn cicd_kind(rel: &str, name: &str, lower: &str, ext: &str) -> Option<String> {
    if rel.contains(".github/workflows/") && (ext == "yml" || ext == "yaml") {
        return Some("github-actions".to_string());
    }
    if rel.contains("/.gitlab-ci.yml") || lower == ".gitlab-ci.yml" {
        return Some("gitlab-ci".to_string());
    }
    if lower == "circleci" || rel.contains("/.circleci/") {
        return Some("circleci".to_string());
    }
    if lower == "jenkinsfile" {
        return Some("jenkins".to_string());
    }
    let _ = name;
    None
}

/// Map a file to an entry-point signal.
fn entry_point_kind(name: &str, lower: &str, ext: &str) -> Option<String> {
    if lower == "makefile" {
        return Some("make".to_string());
    }
    if lower == "justfile" {
        return Some("just".to_string());
    }
    if lower == "cargo.toml" {
        return Some("cargo-bin".to_string());
    }
    if lower == "package.json" {
        return Some("npm-script".to_string());
    }
    if ext == "rs" && lower == "main.rs" {
        return Some("rust-main".to_string());
    }
    let _ = name;
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(rel: &str, is_dir: bool) -> WalkEntry {
        WalkEntry {
            abs: std::path::PathBuf::from(rel),
            rel: rel.to_string(),
            is_dir,
        }
    }

    #[test]
    fn classifies_manifests_and_cicd() {
        let sigs = classify(&entry("Cargo.toml", false));
        assert!(sigs.contains(&Signal::Manifest {
            kind: "cargo".to_string(),
            path: "Cargo.toml".to_string()
        }));
        assert!(sigs.contains(&Signal::EntryPoint {
            kind: "cargo-bin".to_string(),
            path: "Cargo.toml".to_string()
        }));

        let sigs = classify(&entry(".github/workflows/ci.yml", false));
        assert!(
            sigs.iter()
                .any(|s| matches!(s, Signal::Cicd { name, .. } if name == "github-actions"))
        );
    }

    #[test]
    fn secret_classification_is_path_only() {
        for p in [
            ".env",
            "id_rsa.key",
            "credentials.json",
            "secret.yaml",
            "x.pem",
            "y.key",
        ] {
            let sigs = classify(&entry(p, false));
            assert!(
                sigs.iter().any(|s| matches!(s, Signal::Sensitive { .. })),
                "expected {p} to be secret-suspect"
            );
        }
    }

    #[test]
    fn dockerfile_is_container_signal() {
        let sigs = classify(&entry("Dockerfile", false));
        assert!(
            sigs.iter()
                .any(|s| matches!(s, Signal::Container { kind, .. } if kind == "dockerfile"))
        );
    }

    #[test]
    fn ordinary_file_has_no_secret_signal() {
        let sigs = classify(&entry("src/main.rs", false));
        assert!(!sigs.iter().any(|s| matches!(s, Signal::Sensitive { .. })));
    }
}
