//! Shared serialization across the three output formats (design D6/D9).
//!
//! The single [`Report`](crate::analyze::report::Report) model is rendered by
//! `serde_json` (JSON), `serde_yaml` (YAML), and a hand-written Markdown
//! renderer over the same struct. All collections are already sorted by the
//! aggregation stage, guaranteeing byte-stable, format-parity output.
//! MVP2: default is aggregate-only; `--details` adds sorted supporting file paths.

use crate::analyze::report::Report;
use crate::error::LibraryError;
use crate::format::OutputFormat;
use serde_json::{Map, Value};

/// Render `report` in the requested format.
///
/// `details` gates supporting file-path evidence. When false, aggregates only
/// are emitted (no per-entry file arrays). When true, sorted file paths are
/// included alongside aggregates.
pub(crate) fn render(
    report: &Report,
    format: OutputFormat,
    details: bool,
) -> Result<String, LibraryError> {
    match format {
        OutputFormat::Json => {
            let v = to_value(report, details);
            serde_json::to_string(&v).map_err(|e| LibraryError::Io {
                message: format!("failed to serialize json: {e}"),
            })
        }
        OutputFormat::Yaml => {
            let v = to_value(report, details);
            serde_yaml::to_string(&v).map_err(|e| LibraryError::Io {
                message: format!("failed to serialize yaml: {e}"),
            })
        }
        OutputFormat::Markdown => Ok(render_markdown(report, details)),
    }
}

/// Build a serde Value from the report, conditionally including evidence.
fn to_value(report: &Report, details: bool) -> Value {
    let total_classified: usize = report.languages.iter().map(|e| e.files.len()).sum();
    // Helper to build language objects with proportion note.
    let languages: Vec<Value> = report
        .languages
        .iter()
        .map(|e| {
            let mut m = Map::new();
            m.insert("name".to_string(), Value::String(e.name.clone()));
            let count = e.files.len();
            m.insert("file_count".to_string(), Value::Number(count.into()));
            // file-count approximation proportion over classified files only
            let proportion = if total_classified > 0 {
                count as f64 / total_classified as f64
            } else {
                0.0
            };
            // Round to 4 decimal places for determinism
            let proportion_rounded = (proportion * 10000.0).round() / 10000.0;
            m.insert(
                "proportion_approx".to_string(),
                Value::Number(
                    serde_json::Number::from_f64(proportion_rounded)
                        .expect("proportion_approx is finite 0.0..=1.0"),
                ),
            );
            m.insert(
                "proportion_note".to_string(),
                Value::String("file-count approximation, not Linguist byte analysis".to_string()),
            );
            if details {
                let files: Vec<Value> = e
                    .files
                    .iter()
                    .map(|f| Value::String(f.path.clone()))
                    .collect();
                m.insert("files".to_string(), Value::Array(files));
            }
            Value::Object(m)
        })
        .collect();

    let manifests: Vec<Value> = report
        .manifests
        .iter()
        .map(|e| {
            let mut m = Map::new();
            m.insert("kind".to_string(), Value::String(e.kind.clone()));
            m.insert(
                "file_count".to_string(),
                Value::Number(e.files.len().into()),
            );
            if details {
                let files: Vec<Value> = e
                    .files
                    .iter()
                    .map(|f| Value::String(f.path.clone()))
                    .collect();
                m.insert("files".to_string(), Value::Array(files));
            }
            Value::Object(m)
        })
        .collect();
    let linters: Vec<Value> = report
        .linters
        .iter()
        .map(|e| {
            let mut m = Map::new();
            m.insert("name".to_string(), Value::String(e.name.clone()));
            m.insert(
                "file_count".to_string(),
                Value::Number(e.files.len().into()),
            );
            if details {
                let files: Vec<Value> = e
                    .files
                    .iter()
                    .map(|f| Value::String(f.path.clone()))
                    .collect();
                m.insert("files".to_string(), Value::Array(files));
            }
            Value::Object(m)
        })
        .collect();
    let tests: Vec<Value> = report
        .tests
        .iter()
        .map(|e| {
            let mut m = Map::new();
            m.insert("name".to_string(), Value::String(e.name.clone()));
            m.insert(
                "file_count".to_string(),
                Value::Number(e.files.len().into()),
            );
            if details {
                let files: Vec<Value> = e
                    .files
                    .iter()
                    .map(|f| Value::String(f.path.clone()))
                    .collect();
                m.insert("files".to_string(), Value::Array(files));
            }
            Value::Object(m)
        })
        .collect();
    let containers: Vec<Value> = report
        .containers
        .iter()
        .map(|e| {
            let mut m = Map::new();
            m.insert("kind".to_string(), Value::String(e.kind.clone()));
            m.insert(
                "file_count".to_string(),
                Value::Number(e.files.len().into()),
            );
            if details {
                let files: Vec<Value> = e
                    .files
                    .iter()
                    .map(|f| Value::String(f.path.clone()))
                    .collect();
                m.insert("files".to_string(), Value::Array(files));
            }
            Value::Object(m)
        })
        .collect();
    let cicd: Vec<Value> = report
        .cicd
        .iter()
        .map(|e| {
            let mut m = Map::new();
            m.insert("name".to_string(), Value::String(e.name.clone()));
            m.insert(
                "file_count".to_string(),
                Value::Number(e.files.len().into()),
            );
            if details {
                let files: Vec<Value> = e
                    .files
                    .iter()
                    .map(|f| Value::String(f.path.clone()))
                    .collect();
                m.insert("files".to_string(), Value::Array(files));
            }
            Value::Object(m)
        })
        .collect();
    let entry_points: Vec<Value> = report
        .entry_points
        .iter()
        .map(|e| {
            let mut m = Map::new();
            m.insert("kind".to_string(), Value::String(e.kind.clone()));
            m.insert(
                "file_count".to_string(),
                Value::Number(e.files.len().into()),
            );
            if details {
                let files: Vec<Value> = e
                    .files
                    .iter()
                    .map(|f| Value::String(f.path.clone()))
                    .collect();
                m.insert("files".to_string(), Value::Array(files));
            }
            Value::Object(m)
        })
        .collect();

    // Sensitive files are file-level evidence and therefore gated by `details` per the
    // aggregate-first contract (spec: Default SHALL NOT contain per-language/
    // per-technology/per-pattern evidence arrays). They mirror the Markdown gating
    // where default shows only a count summary.
    let sensitive_files = if details {
        let v: Vec<Value> = report
            .sensitive_files
            .iter()
            .map(|e| {
                let mut m = Map::new();
                m.insert("path".to_string(), Value::String(e.path.clone()));
                m.insert("signal".to_string(), Value::String(e.signal.clone()));
                Value::Object(m)
            })
            .collect();
        Value::Array(v)
    } else {
        // Aggregate-only: no per-file objects; keep key present as empty array for parity.
        Value::Array(vec![])
    };

    let limitations: Vec<Value> = report
        .limitations
        .iter()
        .map(|e| {
            let mut m = Map::new();
            m.insert("reason".to_string(), Value::String(e.reason.clone()));
            Value::Object(m)
        })
        .collect();
    let suggestions: Vec<Value> = report
        .suggestions
        .iter()
        .map(|e| {
            let mut m = Map::new();
            m.insert("text".to_string(), Value::String(e.text.clone()));
            Value::Object(m)
        })
        .collect();

    let mut tech = Map::new();
    tech.insert(
        "structure".to_string(),
        Value::Array(
            report
                .technology_patterns
                .structure
                .iter()
                .map(|s| Value::String(s.clone()))
                .collect(),
        ),
    );
    tech.insert(
        "project_shape".to_string(),
        Value::String(report.technology_patterns.project_shape.clone()),
    );
    tech.insert(
        "cicd".to_string(),
        Value::Array(
            report
                .technology_patterns
                .cicd
                .iter()
                .map(|s| Value::String(s.clone()))
                .collect(),
        ),
    );
    tech.insert(
        "infra".to_string(),
        Value::Array(
            report
                .technology_patterns
                .infra
                .iter()
                .map(|s| Value::String(s.clone()))
                .collect(),
        ),
    );
    tech.insert(
        "db".to_string(),
        Value::Array(
            report
                .technology_patterns
                .db
                .iter()
                .map(|s| Value::String(s.clone()))
                .collect(),
        ),
    );
    if details {
        if let Some(ev) = &report.technology_patterns.evidence {
            let mut ev_map = Map::new();
            for (k, v) in ev {
                ev_map.insert(
                    k.clone(),
                    Value::Array(v.iter().map(|f| Value::String(f.path.clone())).collect()),
                );
            }
            tech.insert("evidence".to_string(), Value::Object(ev_map));
        }
    }
    let mut root = Map::new();
    root.insert("technology_patterns".to_string(), Value::Object(tech));
    root.insert("languages".to_string(), Value::Array(languages));
    root.insert("manifests".to_string(), Value::Array(manifests));
    root.insert("linters".to_string(), Value::Array(linters));
    root.insert("tests".to_string(), Value::Array(tests));
    root.insert("containers".to_string(), Value::Array(containers));
    root.insert("cicd".to_string(), Value::Array(cicd));
    root.insert("entry_points".to_string(), Value::Array(entry_points));
    root.insert("sensitive_files".to_string(), sensitive_files);
    root.insert("limitations".to_string(), Value::Array(limitations));
    root.insert("suggestions".to_string(), Value::Array(suggestions));
    Value::Object(root)
}

/// Hand-written Markdown renderer over the same `Report` struct.
///
/// `details` controls whether supporting file paths are shown. Aggregate
/// information (counts, proportions) is always shown.
pub(crate) fn render_markdown(report: &Report, details: bool) -> String {
    let mut out = String::new();
    out.push_str("# Repository Analysis\n\n");
    // Language section includes file-count approximation note
    if !report.languages.is_empty() {
        let total: usize = report.languages.iter().map(|e| e.files.len()).sum();
        out.push_str("## Languages\n\n");
        out.push_str("_Language proportions are file-count approximations over classified files only (not Linguist byte analysis)._\n\n");
        for e in &report.languages {
            let count = e.files.len();
            let prop = if total > 0 {
                count as f64 / total as f64 * 100.0
            } else {
                0.0
            };
            if details {
                out.push_str(&format!(
                    "- {} ({} files, {:.1}% — {}) : `{}`\n",
                    e.name,
                    count,
                    prop,
                    "file-count approx",
                    paths_join(&e.files)
                ));
            } else {
                out.push_str(&format!(
                    "- {} ({} files, {:.1}% — file-count approximation)\n",
                    e.name, count, prop
                ));
            }
        }
        out.push('\n');
    }

    // Helper for other categories — avoids intermediate Vec allocation (Chapter 3: zero-cost iterators).
    // Takes iterators borrowing the report so no cloning of Vec<FileRef> occurs.
    let mut render_category =
        |title: &str,
         iter: &mut dyn Iterator<Item = (&str, &[crate::analyze::report::FileRef])>| {
            let mut first = true;
            for (name, files) in iter {
                if first {
                    out.push_str(title);
                    out.push_str("\n\n");
                    first = false;
                }
                if details {
                    out.push_str(&format!(
                        "- {} ({} files) (`{}`)\n",
                        name,
                        files.len(),
                        paths_join(files)
                    ));
                } else {
                    out.push_str(&format!("- {} ({} files)\n", name, files.len()));
                }
            }
            if !first {
                out.push('\n');
            }
        };

    render_category(
        "## Manifests",
        &mut report
            .manifests
            .iter()
            .map(|e| (e.kind.as_str(), e.files.as_slice())),
    );
    render_category(
        "## Linters",
        &mut report
            .linters
            .iter()
            .map(|e| (e.name.as_str(), e.files.as_slice())),
    );
    render_category(
        "## Tests",
        &mut report
            .tests
            .iter()
            .map(|e| (e.name.as_str(), e.files.as_slice())),
    );
    render_category(
        "## Containers",
        &mut report
            .containers
            .iter()
            .map(|e| (e.kind.as_str(), e.files.as_slice())),
    );
    render_category(
        "## CI/CD",
        &mut report
            .cicd
            .iter()
            .map(|e| (e.name.as_str(), e.files.as_slice())),
    );
    render_category(
        "## Entry Points",
        &mut report
            .entry_points
            .iter()
            .map(|e| (e.kind.as_str(), e.files.as_slice())),
    );

    // Technology patterns section (always show aggregates, evidence gated)
    {
        out.push_str("## Technology Patterns\n\n");
        out.push_str(&format!(
            "- project_shape: {}\n",
            report.technology_patterns.project_shape
        ));
        if !report.technology_patterns.structure.is_empty() {
            out.push_str(&format!(
                "- structure: {}\n",
                report.technology_patterns.structure.join(", ")
            ));
        } else {
            out.push_str("- structure: (none)\n");
        }
        if !report.technology_patterns.cicd.is_empty() {
            out.push_str(&format!(
                "- cicd: {}\n",
                report.technology_patterns.cicd.join(", ")
            ));
        } else {
            out.push_str("- cicd: (none)\n");
        }
        if !report.technology_patterns.infra.is_empty() {
            out.push_str(&format!(
                "- infra: {}\n",
                report.technology_patterns.infra.join(", ")
            ));
        } else {
            out.push_str("- infra: (none)\n");
        }
        if !report.technology_patterns.db.is_empty() {
            out.push_str(&format!(
                "- db: {}\n",
                report.technology_patterns.db.join(", ")
            ));
        } else {
            out.push_str("- db: (none)\n");
        }
        if details {
            if let Some(ev) = &report.technology_patterns.evidence {
                for (k, paths) in ev {
                    out.push_str(&format!(
                        "- evidence {}: {}\n",
                        k,
                        paths
                            .iter()
                            .map(|p| p.path.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
        }
        out.push('\n');
    }

    if details && !report.sensitive_files.is_empty() {
        out.push_str("## Sensitive Files\n\n");
        for e in &report.sensitive_files {
            out.push_str(&format!("- `{}` (signal: {})\n", e.path, e.signal));
        }
        out.push('\n');
    } else if !details && !report.sensitive_files.is_empty() {
        out.push_str("## Sensitive Files\n\n");
        out.push_str(&format!(
            "- {} sensitive file(s) detected (details hidden; use --details)\n\n",
            report.sensitive_files.len()
        ));
    }

    if !report.suggestions.is_empty() {
        out.push_str("## Suggestions\n\n");
        for s in &report.suggestions {
            out.push_str("- ");
            out.push_str(&s.text);
            out.push('\n');
        }
        out.push('\n');
    }

    if !report.limitations.is_empty() {
        out.push_str("## Limitations\n\n");
        for l in &report.limitations {
            out.push_str("- ");
            out.push_str(&l.reason);
            out.push('\n');
        }
        out.push('\n');
    }

    // If no sections emitted, keep heading only (still valid)
    out
}

/// Render a named section with its bullet list, if non-empty.
#[allow(dead_code)]
fn section_list<T, F>(out: &mut String, title: &str, items: &[T], fmt: F)
where
    F: Fn(&T) -> String,
{
    if items.is_empty() {
        return;
    }
    out.push_str(title);
    out.push_str("\n\n");
    for item in items {
        out.push_str("- ");
        out.push_str(&fmt(item));
        out.push('\n');
    }
    out.push('\n');
}

/// Join FileRef paths with commas for inline display.
fn paths_join(files: &[crate::analyze::report::FileRef]) -> String {
    let mut out = String::new();
    for (i, f) in files.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&f.path);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::report::{FileRef, Report};

    fn sample() -> Report {
        let mut r = Report::new();
        r.languages.push(crate::analyze::report::LanguageEntry {
            name: "Rust".to_string(),
            files: vec![FileRef {
                path: "src/main.rs".to_string(),
            }],
        });
        r.sensitive_files
            .push(crate::analyze::report::SensitiveEntry {
                path: ".env".to_string(),
                signal: "env-file".to_string(),
            });
        r
    }

    #[test]
    fn json_yaml_round_trip_same_model() {
        let r = sample();
        let json = render(&r, OutputFormat::Json, true).expect("json");
        let yaml = render(&r, OutputFormat::Yaml, true).expect("yaml");
        assert!(json.contains("\"languages\""));
        assert!(yaml.contains("languages:"));
        assert!(json.contains("sensitive_files"));
        assert!(yaml.contains("sensitive_files"));
    }

    #[test]
    fn markdown_contains_sections() {
        let r = sample();
        let md = render_markdown(&r, true);
        assert!(md.contains("## Languages"));
        assert!(md.contains("Rust"));
        assert!(md.contains(".env"));
        assert!(md.contains("signal: env-file"));
    }

    #[test]
    fn empty_report_has_all_keys_in_json() {
        let r = Report::new();
        let json = render(&r, OutputFormat::Json, false).expect("json");
        for key in [
            "languages",
            "manifests",
            "linters",
            "tests",
            "containers",
            "cicd",
            "entry_points",
            "sensitive_files",
            "limitations",
            "suggestions",
        ] {
            assert!(json.contains(key), "missing key {key}");
        }
    }

    #[test]
    fn default_omits_files_details_includes_them() {
        let r = sample();
        let json_default = render(&r, OutputFormat::Json, false).unwrap();
        assert!(
            !json_default.contains("src/main.rs"),
            "default should omit evidence"
        );
        // file_count aggregate present
        assert!(json_default.contains("file_count"));
        let json_details = render(&r, OutputFormat::Json, true).unwrap();
        assert!(
            json_details.contains("src/main.rs"),
            "details should include evidence"
        );
    }

    #[test]
    fn language_proportion_note_present() {
        let r = sample();
        let json = render(&r, OutputFormat::Json, false).unwrap();
        assert!(json.contains("file-count approximation"));
        // Must not imply byte-based Linguist analysis
        assert!(!json.contains("Linguist byte") || json.contains("not Linguist"));
        let md = render_markdown(&r, false);
        assert!(md.contains("file-count approximation"));
        assert!(md.contains("not Linguist"));
    }

    #[test]
    fn sensitive_files_gated_by_details() {
        let r = sample();
        // Default (details=false): aggregate-only, no per-file evidence
        let json_default = render(&r, OutputFormat::Json, false).unwrap();
        assert!(
            !json_default.contains(".env"),
            "default must not leak sensitive file paths"
        );
        assert!(
            !json_default.contains("env-file"),
            "default must not leak sensitive signals"
        );
        let md_default = render_markdown(&r, false);
        assert!(!md_default.contains(".env"));
        assert!(md_default.contains("sensitive file(s) detected"));
        // Details=true: evidence deterministically included
        let json_details = render(&r, OutputFormat::Json, true).unwrap();
        assert!(json_details.contains(".env"));
        assert!(json_details.contains("env-file"));
        let md_details = render_markdown(&r, true);
        assert!(md_details.contains(".env"));
        assert!(md_details.contains("env-file"));
    }
}
