//! Shared report model for repository analysis.
//!
//! One `Report` struct is the single source of truth for all three output
//! formats (Markdown/JSON/YAML). Every category is a collection that is emitted
//! as an explicit (possibly empty) key, never omitted, so partial/incomplete
//! scans stay well-formed and format parity holds (design D6).

use serde::Serialize;

/// A single typed file-system fact derived from a path only (no contents read).
///
/// Relative paths are normalized to repository-relative forward-slash form.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct FileRef {
    /// Normalized repository-relative path (forward slashes, no `.`/`..`).
    pub(crate) path: String,
}

/// A detected language signal with the files that evidence it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct LanguageEntry {
    /// Canonical language name (e.g. "Rust", "JavaScript").
    pub(crate) name: String,
    /// Normalized evidence paths confirming the language.
    pub(crate) files: Vec<FileRef>,
}

/// A detected manifest (build/dependency descriptor).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct ManifestEntry {
    /// Manifest kind name (e.g. "cargo", "npm", "go", "python").
    pub(crate) kind: String,
    /// Normalized paths of the manifest files.
    pub(crate) files: Vec<FileRef>,
}

/// A detected linter/static-analysis configuration.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct LinterEntry {
    /// Linter/tool name (e.g. "clippy", "eslint").
    pub(crate) name: String,
    /// Normalized config paths.
    pub(crate) files: Vec<FileRef>,
}

/// A detected test signal.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct TestEntry {
    /// Test framework/group name (e.g. "cargo-test", "pytest", "jest").
    pub(crate) name: String,
    /// Normalized evidence paths.
    pub(crate) files: Vec<FileRef>,
}

/// A detected container signal.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct ContainerEntry {
    /// Container kind (e.g. "dockerfile", "docker-compose").
    pub(crate) kind: String,
    /// Normalized evidence paths.
    pub(crate) files: Vec<FileRef>,
}

/// A detected CI/CD signal.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct CicdEntry {
    /// CI/CD system name (e.g. "github-actions", "gitlab-ci").
    pub(crate) name: String,
    /// Normalized evidence paths.
    pub(crate) files: Vec<FileRef>,
}

/// A detected build/run entry point.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct EntryPointEntry {
    /// Entry point kind (e.g. "make", "cargo-bin", "npm-script").
    pub(crate) kind: String,
    /// Normalized evidence paths.
    pub(crate) files: Vec<FileRef>,
}

/// A secret-suspect file classified by path/name signal only.
///
/// No contents, size, or bytes are ever read or serialized (design D5).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct SensitiveEntry {
    /// Normalized repository-relative path of the suspect file.
    pub(crate) path: String,
    /// Standard signal label explaining *why* it was flagged (e.g. "env-file").
    pub(crate) signal: String,
}

/// A catalog guidance suggestion produced by `--suggest`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct SuggestionEntry {
    /// Catalog line text.
    pub(crate) text: String,
}

/// A limitation / fail-soft note recorded during the scan.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct LimitationEntry {
    /// Stable human-readable reason (e.g. "unreadable directory: <path>").
    pub(crate) reason: String,
}

/// The aggregated repository analysis report.
///
/// Serialized once and rendered by three serializers (design D6). All
/// collections are sorted so output is byte-stable (design D9).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Report {
    /// Detected programming languages.
    pub(crate) languages: Vec<LanguageEntry>,
    /// Detected build/dependency manifests.
    pub(crate) manifests: Vec<ManifestEntry>,
    /// Detected linter/static-analysis configs.
    pub(crate) linters: Vec<LinterEntry>,
    /// Detected test signals.
    pub(crate) tests: Vec<TestEntry>,
    /// Detected container signals.
    pub(crate) containers: Vec<ContainerEntry>,
    /// Detected CI/CD signals.
    pub(crate) cicd: Vec<CicdEntry>,
    /// Detected build/run entry points.
    pub(crate) entry_points: Vec<EntryPointEntry>,
    /// Secret-suspect files (path + label only, never contents).
    pub(crate) sensitive_files: Vec<SensitiveEntry>,
    /// Limitations / fail-soft notes encountered during the scan.
    pub(crate) limitations: Vec<LimitationEntry>,
    /// Optional `--suggest` catalog guidance lines.
    pub(crate) suggestions: Vec<SuggestionEntry>,
}

impl Report {
    /// A new, empty report with every category present as an empty collection.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Whether the report carries no detected evidence at all.
    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.languages.is_empty()
            && self.manifests.is_empty()
            && self.linters.is_empty()
            && self.tests.is_empty()
            && self.containers.is_empty()
            && self.cicd.is_empty()
            && self.entry_points.is_empty()
            && self.sensitive_files.is_empty()
    }

    /// Sort every internal collection lexicographically (design D9).
    ///
    /// Called once before serialization to guarantee byte-stable output.
    pub(crate) fn sort_all(&mut self) {
        self.languages.sort();
        self.manifests.sort();
        self.linters.sort();
        self.tests.sort();
        self.containers.sort();
        self.cicd.sort();
        self.entry_points.sort();
        self.sensitive_files.sort();
        self.limitations.sort();
        self.suggestions.sort();
        for l in &mut self.languages {
            l.files.sort();
        }
        for m in &mut self.manifests {
            m.files.sort();
        }
        for l in &mut self.linters {
            l.files.sort();
        }
        for t in &mut self.tests {
            t.files.sort();
        }
        for c in &mut self.containers {
            c.files.sort();
        }
        for c in &mut self.cicd {
            c.files.sort();
        }
        for e in &mut self.entry_points {
            e.files.sort();
        }
    }
}
