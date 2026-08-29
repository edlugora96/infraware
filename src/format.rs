//! Typed output-format value shared across the CLI surface.

use std::fmt;

/// Output format for structured command data.
///
/// Exactly three formats are supported. [`OutputFormat::Markdown`] is the
/// default when a command accepts `--format` but the caller omits it.
///
/// JSON is the primary machine-readable projection; YAML is output-only and is
/// never a canonical storage format. These semantics are frozen by the
/// foundation and consumed by later plans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OutputFormat {
    /// Human-readable Markdown (default when `--format` is omitted).
    #[default]
    Markdown,
    /// Machine-readable JSON projection.
    Json,
    /// Output-only YAML (never canonical storage).
    Yaml,
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            OutputFormat::Markdown => "markdown",
            OutputFormat::Json => "json",
            OutputFormat::Yaml => "yaml",
        };
        f.write_str(name)
    }
}

impl OutputFormat {
    /// Parse an exact `--format` value token.
    ///
    /// Only `markdown`, `json`, and `yaml` are accepted; any other value
    /// (including `xml`) returns [`None`] so the parser can reject it as a
    /// usage error.
    pub fn parse(token: &str) -> Option<OutputFormat> {
        match token {
            "markdown" => Some(OutputFormat::Markdown),
            "json" => Some(OutputFormat::Json),
            "yaml" => Some(OutputFormat::Yaml),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_markdown() {
        assert_eq!(OutputFormat::default(), OutputFormat::Markdown);
    }

    #[test]
    fn parse_accepts_exact_formats() {
        assert_eq!(
            OutputFormat::parse("markdown"),
            Some(OutputFormat::Markdown)
        );
        assert_eq!(OutputFormat::parse("json"), Some(OutputFormat::Json));
        assert_eq!(OutputFormat::parse("yaml"), Some(OutputFormat::Yaml));
    }

    #[test]
    fn parse_rejects_invalid_formats() {
        assert_eq!(OutputFormat::parse("xml"), None);
        assert_eq!(OutputFormat::parse("JSON"), None);
        assert_eq!(OutputFormat::parse("markdown "), None);
        assert_eq!(OutputFormat::parse(""), None);
    }

    #[test]
    fn display_round_trips_exact_values() {
        assert_eq!(OutputFormat::Markdown.to_string(), "markdown");
        assert_eq!(OutputFormat::Json.to_string(), "json");
        assert_eq!(OutputFormat::Yaml.to_string(), "yaml");
    }
}
