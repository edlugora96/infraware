//! Typed output-format value shared across the CLI surface.

use std::fmt;

/// Output format for structured command data.
///
/// The SQLite-backed verbs render JSON. YAML remains as an output-only
/// projection; it is never a canonical storage format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OutputFormat {
    /// Machine-readable JSON projection (the default).
    #[default]
    Json,
    /// Output-only YAML (never canonical storage).
    Yaml,
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            OutputFormat::Json => "json",
            OutputFormat::Yaml => "yaml",
        };
        f.write_str(name)
    }
}

impl OutputFormat {
    /// Parse an exact `--format` value token.
    ///
    /// Only `json` and `yaml` are accepted; any other value (including `xml`)
    /// returns [`None`] so the parser can reject it as a usage error.
    pub fn parse(token: &str) -> Option<OutputFormat> {
        match token {
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
    fn default_is_json() {
        assert_eq!(OutputFormat::default(), OutputFormat::Json);
    }

    #[test]
    fn parse_accepts_exact_formats() {
        assert_eq!(OutputFormat::parse("json"), Some(OutputFormat::Json));
        assert_eq!(OutputFormat::parse("yaml"), Some(OutputFormat::Yaml));
    }

    #[test]
    fn parse_rejects_invalid_formats() {
        assert_eq!(OutputFormat::parse("xml"), None);
        assert_eq!(OutputFormat::parse("JSON"), None);
        assert_eq!(OutputFormat::parse("json "), None);
        assert_eq!(OutputFormat::parse(""), None);
    }

    #[test]
    fn display_round_trips_exact_values() {
        assert_eq!(OutputFormat::Json.to_string(), "json");
        assert_eq!(OutputFormat::Yaml.to_string(), "yaml");
    }
}
