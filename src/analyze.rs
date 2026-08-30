//! Offline, deterministic repository analysis pipeline (design D1/D2).
//!
//! Reached exclusively through `Command::Analyze` in [`dispatch`]. The pipeline
//! is strictly one-directional:
//!
//! ```text
//! traversal → signals → report aggregation → (optional) suggest → serialize
//! ```
//!
//! All stages are read-only and O(paths)-bounded; no file contents are read for
//! classification (design D4/D5). Output is byte-stable (design D9) and empty/
//! incomplete/partially-unreadable repositories still succeed (design D8).

pub(crate) mod report;
pub(crate) mod serialize;
pub(crate) mod signals;
pub(crate) mod suggest;
pub(crate) mod technology;
pub(crate) mod traversal;

use crate::analyze::report::{
    CicdEntry, ContainerEntry, EntryPointEntry, FileRef, LanguageEntry, LimitationEntry,
    LinterEntry, ManifestEntry, Report, SensitiveEntry, SuggestionEntry, TestEntry,
};
use crate::analyze::signals::{Signal, SignalGroups};
use crate::analyze::traversal::{WalkLimit, WalkResult};
use crate::error::LibraryError;
use crate::format::OutputFormat;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;

// `Permissions::from_mode` is used only by the `#[cfg(test)]` unit tests on
// Unix targets and is provided by this trait; gate the import so it is not
// flagged as unused when the library itself is compiled without tests.
#[cfg(all(unix, test))]
use std::os::unix::fs::PermissionsExt;

/// Run the full analysis pipeline over `root` and render the report.
///
/// Returns the structured body for stdout. Never fails for unreadable subtrees;
/// those become `limitations` and the scan still yields a valid report with a
/// `0` status (design D8). `suggest` toggles `--suggest` catalog guidance,
/// `details` toggles file-path evidence (aggregate-only when false).
pub(crate) fn run(
    root: &Path,
    format: OutputFormat,
    suggest: bool,
    details: bool,
) -> Result<String, LibraryError> {
    let root = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| LibraryError::Io {
                message: format!("failed to resolve current directory: {e}"),
            })?
            .join(root)
            .canonicalize()
            .map_err(|e| LibraryError::Io {
                message: format!("failed to canonicalize root: {e}"),
            })?
    };

    let walked = traversal::walk(&root)?;
    let mut report = aggregate(&walked, details);
    if suggest {
        let groups = collect_groups(&walked);
        let texts = suggest::select_suggestions(&groups);
        report.suggestions = texts
            .into_iter()
            .map(|text| SuggestionEntry { text })
            .collect();
    }
    report.sort_all();
    serialize::render(&report, format, details)
}

/// Aggregate the walked entries into a shared [`Report`].
fn aggregate(walked: &WalkResult, details: bool) -> Report {
    let mut report = Report::new();

    // Group evidence files by signal name for stable, deduplicated aggregation.
    let mut languages: BTreeMap<String, BTreeSet<FileRef>> = BTreeMap::new();
    let mut manifests: BTreeMap<String, BTreeSet<FileRef>> = BTreeMap::new();
    let mut linters: BTreeMap<String, BTreeSet<FileRef>> = BTreeMap::new();
    let mut tests: BTreeMap<String, BTreeSet<FileRef>> = BTreeMap::new();
    let mut containers: BTreeMap<String, BTreeSet<FileRef>> = BTreeMap::new();
    let mut cicd: BTreeMap<String, BTreeSet<FileRef>> = BTreeMap::new();
    let mut entry_points: BTreeMap<String, BTreeSet<FileRef>> = BTreeMap::new();
    let mut sensitive: BTreeSet<SensitiveEntry> = BTreeSet::new();

    for entry in &walked.entries {
        // Directories carry no file-content evidence; skip classifying dirs
        // for file-type signals (they are only relevant for skip detection).
        let signals = signals::classify(entry);
        for sig in signals {
            let path = FileRef {
                path: sig_path(&sig).to_string(),
            };
            match &sig {
                Signal::Language { name, .. } => {
                    languages.entry(name.clone()).or_default().insert(path);
                }
                Signal::Manifest { kind, .. } => {
                    manifests.entry(kind.clone()).or_default().insert(path);
                }
                Signal::Linter { name, .. } => {
                    linters.entry(name.clone()).or_default().insert(path);
                }
                Signal::Test { name, .. } => {
                    tests.entry(name.clone()).or_default().insert(path);
                }
                Signal::Container { kind, .. } => {
                    containers.entry(kind.clone()).or_default().insert(path);
                }
                Signal::Cicd { name, .. } => {
                    cicd.entry(name.clone()).or_default().insert(path);
                }
                Signal::EntryPoint { kind, .. } => {
                    entry_points.entry(kind.clone()).or_default().insert(path);
                }
                Signal::Sensitive { path, signal } => {
                    sensitive.insert(SensitiveEntry {
                        path: path.clone(),
                        signal: signal.clone(),
                    });
                }
            }
        }
    }

    report.languages = drain(languages, |name, files| LanguageEntry { name, files });
    report.manifests = drain(manifests, |kind, files| ManifestEntry { kind, files });
    report.linters = drain(linters, |name, files| LinterEntry { name, files });
    report.tests = drain(tests, |name, files| TestEntry { name, files });
    report.containers = drain(containers, |kind, files| ContainerEntry { kind, files });
    report.cicd = drain(cicd, |name, files| CicdEntry { name, files });
    report.entry_points = drain(entry_points, |kind, files| EntryPointEntry { kind, files });
    report.sensitive_files = sensitive.into_iter().collect();

    report.limitations = walked
        .limits
        .iter()
        .map(|l: &WalkLimit| LimitationEntry {
            reason: l.reason.clone(),
        })
        .collect();

    // Technology patterns (mvp2) - path-only deterministic, zero intermediate
    // allocations via iterators (Chapter 3).
    {
        let collected = technology::collect_patterns(&walked.entries);
        // BTreeSet keys are already sorted; collect without extra sort where possible.
        let structure_vec: Vec<String> = collected.structure.into_iter().collect();
        let cicd_vec: Vec<String> = collected.cicd.keys().cloned().collect();
        let infra_vec: Vec<String> = collected.infra.keys().cloned().collect();
        let db_vec: Vec<String> = collected.db.keys().cloned().collect();
        let shape = if walked.entries.is_empty() {
            "undetermined".to_string()
        } else {
            technology::project_shape(
                collected.workspace_found,
                collected.group_dir_found,
                &collected.manifest_dirs,
            )
        };
        report.technology_patterns.structure = structure_vec;
        report.technology_patterns.project_shape = shape;
        report.technology_patterns.cicd = cicd_vec;
        report.technology_patterns.infra = infra_vec;
        report.technology_patterns.db = db_vec;
        if details {
            let mut evidence: BTreeMap<String, Vec<FileRef>> = BTreeMap::new();
            let mut struct_ev: BTreeSet<String> = BTreeSet::new();
            for e in &walked.entries {
                if technology::match_structure(&e.rel, e.is_dir).is_some() {
                    struct_ev.insert(e.rel.clone());
                }
            }
            if !struct_ev.is_empty() {
                evidence.insert(
                    "structure".to_string(),
                    struct_ev.into_iter().map(|p| FileRef { path: p }).collect(),
                );
            }
            for (k, set) in collected.cicd {
                evidence.insert(
                    format!("cicd:{k}"),
                    set.into_iter().map(|p| FileRef { path: p }).collect(),
                );
            }
            for (k, set) in collected.infra {
                evidence.insert(
                    format!("infra:{k}"),
                    set.into_iter().map(|p| FileRef { path: p }).collect(),
                );
            }
            for (k, set) in collected.db {
                evidence.insert(
                    format!("db:{k}"),
                    set.into_iter().map(|p| FileRef { path: p }).collect(),
                );
            }
            report.technology_patterns.evidence = if evidence.is_empty() {
                None
            } else {
                Some(evidence)
            };
        } else {
            report.technology_patterns.evidence = None;
        }
    }

    report
}

/// Collect the observed signal groups for `--suggest` (design D7).
fn collect_groups(walked: &WalkResult) -> SignalGroups {
    let mut groups = SignalGroups::default();
    for entry in &walked.entries {
        for sig in signals::classify(entry) {
            match sig {
                Signal::Language { name, .. } => {
                    groups.languages.insert(name);
                }
                Signal::Manifest { kind, .. } => {
                    groups.manifests.insert(kind);
                }
                Signal::Linter { name, .. } => {
                    groups.linters.insert(name);
                }
                Signal::Test { name, .. } => {
                    groups.tests.insert(name);
                }
                Signal::Container { kind, .. } => {
                    groups.containers.insert(kind);
                }
                Signal::Cicd { name, .. } => {
                    groups.cicd.insert(name);
                }
                Signal::EntryPoint { kind, .. } => {
                    groups.entry_points.insert(kind);
                }
                Signal::Sensitive { .. } => {
                    groups.sensitive = true;
                }
            }
        }
    }
    groups
}

/// Drain a grouped map into a sorted `Vec` of entries.
fn drain<T>(
    map: BTreeMap<String, BTreeSet<FileRef>>,
    build: impl Fn(String, Vec<FileRef>) -> T,
) -> Vec<T> {
    map.into_iter()
        .map(|(name, set)| build(name, set.into_iter().collect()))
        .collect()
}

/// Extract the relative path carried by a signal.
fn sig_path(sig: &Signal) -> &str {
    match sig {
        Signal::Language { path, .. }
        | Signal::Manifest { path, .. }
        | Signal::Linter { path, .. }
        | Signal::Test { path, .. }
        | Signal::Container { path, .. }
        | Signal::Cicd { path, .. }
        | Signal::EntryPoint { path, .. }
        | Signal::Sensitive { path, .. } => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// RAII temp-root guard mirroring `dispatch.rs` test pattern.
    struct TempRoot(std::path::PathBuf);
    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn temp_root() -> TempRoot {
        let base = std::env::temp_dir().join(format!(
            "infraware-analyze-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&base).expect("create temp root");
        TempRoot(base)
    }

    #[test]
    fn empty_repo_yields_valid_report_all_keys() {
        let root = temp_root();
        let body = run(&root.0, OutputFormat::Json, false, false).expect("analyze empty");
        assert!(body.contains("\"languages\""));
        assert!(body.contains("\"sensitive_files\""));
        assert!(body.contains("\"suggestions\""));
    }

    #[test]
    fn typical_repo_lists_manifests_and_cicd() {
        let root = temp_root();
        fs::write(root.0.join("Cargo.toml"), "[package]\n").unwrap();
        fs::write(root.0.join("package.json"), "{}").unwrap();
        fs::create_dir_all(root.0.join(".github/workflows")).unwrap();
        fs::write(root.0.join(".github/workflows/ci.yml"), "on: push\n").unwrap();

        let body = run(&root.0, OutputFormat::Json, false, false).expect("analyze");
        assert!(body.contains("cargo"));
        assert!(body.contains("npm"));
        assert!(body.contains("github-actions"));
    }

    #[test]
    fn secret_file_is_reported_without_contents() {
        let root = temp_root();
        fs::write(root.0.join(".env"), "SECRET=1\n").unwrap();
        let body = run(&root.0, OutputFormat::Json, false, true).expect("analyze");
        assert!(body.contains(".env"));
        assert!(body.contains("env-file"));
        assert!(!body.contains("SECRET=1"), "secret value must not leak");
    }

    #[test]
    fn unreadable_dir_becomes_limitation_status_ok() {
        let root = temp_root();
        let locked = root.0.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::write(locked.join("f.txt"), "x").unwrap();
        let _ = fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000));

        // Best-effort: permission-denied dirs may or may not be traversable
        // depending on platform/root; assert no panic and a valid report.
        let body = run(&root.0, OutputFormat::Json, false, false);
        let _ = fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
        assert!(body.is_ok());
    }

    #[test]
    fn determinism_for_repeated_runs() {
        let root = temp_root();
        fs::write(root.0.join("Cargo.toml"), "[package]\n").unwrap();
        fs::write(root.0.join("a.rs"), "").unwrap();
        fs::write(root.0.join("b.rs"), "").unwrap();
        let a = run(&root.0, OutputFormat::Json, true, false).expect("run a");
        let b = run(&root.0, OutputFormat::Json, true, false).expect("run b");
        assert_eq!(a, b, "repeated runs must be byte-identical");
    }
}
