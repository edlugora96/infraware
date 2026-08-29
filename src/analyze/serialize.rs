//! Shared serialization across the three output formats (design D6/D9).
//!
//! The single [`Report`](crate::analyze::report::Report) model is rendered by
//! `serde_json` (JSON), `serde_yaml` (YAML), and a hand-written Markdown
//! renderer over the same struct. All collections are already sorted by the
//! aggregation stage, guaranteeing byte-stable, format-parity output.

use crate::analyze::report::Report;
use crate::error::LibraryError;
use crate::format::OutputFormat;

/// Render `report` in the requested format.
///
/// Returns the structured body destined for stdout. Never returns an error for
/// a well-formed report (serde failures are mapped to `LibraryError::Io`).
pub(crate) fn render(report: &Report, format: OutputFormat) -> Result<String, LibraryError> {
    match format {
        OutputFormat::Json => serde_json::to_string(report).map_err(|e| LibraryError::Io {
            message: format!("failed to serialize json: {e}"),
        }),
        OutputFormat::Yaml => serde_yaml::to_string(report).map_err(|e| LibraryError::Io {
            message: format!("failed to serialize yaml: {e}"),
        }),
        OutputFormat::Markdown => Ok(render_markdown(report)),
    }
}

/// Hand-written Markdown renderer over the same `Report` struct.
///
/// Section order and list ordering are consistent and sorted, so the Markdown
/// view conveys the same information as the machine-readable formats.
pub(crate) fn render_markdown(report: &Report) -> String {
    let mut out = String::new();
    out.push_str("# Repository Analysis\n\n");

    section_list(&mut out, "## Languages", &report.languages, |e| {
        format!("{} (`{}`)", e.name, paths_join(&e.files))
    });
    section_list(&mut out, "## Manifests", &report.manifests, |e| {
        format!("{} (`{}`)", e.kind, paths_join(&e.files))
    });
    section_list(&mut out, "## Linters", &report.linters, |e| {
        format!("{} (`{}`)", e.name, paths_join(&e.files))
    });
    section_list(&mut out, "## Tests", &report.tests, |e| {
        format!("{} (`{}`)", e.name, paths_join(&e.files))
    });
    section_list(&mut out, "## Containers", &report.containers, |e| {
        format!("{} (`{}`)", e.kind, paths_join(&e.files))
    });
    section_list(&mut out, "## CI/CD", &report.cicd, |e| {
        format!("{} (`{}`)", e.name, paths_join(&e.files))
    });
    section_list(&mut out, "## Entry Points", &report.entry_points, |e| {
        format!("{} (`{}`)", e.kind, paths_join(&e.files))
    });
    section_list(
        &mut out,
        "## Sensitive Files",
        &report.sensitive_files,
        |e| format!("`{}` (signal: {})", e.path, e.signal),
    );

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

    out
}

/// Render a named section with its bullet list, if non-empty.
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
    // Build the joined string directly from the iterator to avoid an
    // intermediate allocation (clippy `needless_collect`), preserving output.
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
        let json = render(&r, OutputFormat::Json).expect("json");
        let yaml = render(&r, OutputFormat::Yaml).expect("yaml");
        assert!(json.contains("\"languages\""));
        assert!(yaml.contains("languages:"));
        // Both contain the same category keys.
        assert!(json.contains("sensitive_files"));
        assert!(yaml.contains("sensitive_files"));
    }

    #[test]
    fn markdown_contains_sections() {
        let r = sample();
        let md = render_markdown(&r);
        assert!(md.contains("## Languages"));
        assert!(md.contains("Rust"));
        assert!(md.contains(".env"));
        assert!(md.contains("signal: env-file"));
    }

    #[test]
    fn empty_report_has_all_keys_in_json() {
        let r = Report::new();
        let json = render(&r, OutputFormat::Json).expect("json");
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
}
