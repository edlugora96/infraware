use crate::error::LibraryError;
use serde_yaml::Value;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Settings {
    pub task_dedup: f64,
    pub rule_dedup: f64,
    pub query_max: f64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            task_dedup: 0.15,
            rule_dedup: 0.15,
            query_max: 0.15,
        }
    }
}

pub(crate) fn load(root: &Path) -> Result<Settings, LibraryError> {
    let path = root.join(".infraware/settings.yaml");
    let text = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(e) => return Err(err(format!("cannot read settings: {e}"))),
    };
    if text.trim().is_empty() {
        return Ok(Settings::default());
    }
    let value: Value =
        serde_yaml::from_str(&text).map_err(|e| err(format!("invalid settings: {e}")))?;
    let Some(root) = value.as_mapping() else {
        return if value.is_null() {
            Ok(Settings::default())
        } else {
            Err(err("settings root must be a mapping".into()))
        };
    };
    let Some(dist) = root.get(Value::String("distances".into())) else {
        return Ok(Settings::default());
    };
    let Some(dist) = dist.as_mapping() else {
        return if dist.is_null() {
            Ok(Settings::default())
        } else {
            Err(err("distances must be a mapping".into()))
        };
    };
    let mut out = Settings::default();
    for (key, slot) in [
        ("task_dedup", &mut out.task_dedup),
        ("rule_dedup", &mut out.rule_dedup),
        ("query_max", &mut out.query_max),
    ] {
        let Some(v) = dist.get(Value::String(key.into())) else {
            continue;
        };
        if v.is_null() {
            continue;
        }
        let Some(n) = v.as_f64() else {
            return Err(err(format!("{key} must be a number")));
        };
        if !n.is_finite() || !(0.0..=2.0).contains(&n) {
            return Err(err(format!("{key} must be finite and in [0, 2]")));
        }
        *slot = n;
    }
    Ok(out)
}
fn err(message: String) -> LibraryError {
    LibraryError::Configuration { message }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn repo() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "infraware-settings-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    fn write(root: &Path, body: &str) {
        fs::create_dir_all(root.join(".infraware")).unwrap();
        fs::write(root.join(".infraware/settings.yaml"), body).unwrap();
    }
    #[test]
    fn defaults_are_independent_and_fallbacks_are_supported() {
        let root = repo();
        write(
            &root,
            "distances:\n  task_dedup: 0\n  query_max: null\n  unknown: 2\n",
        );
        assert_eq!(
            load(&root).unwrap(),
            Settings {
                task_dedup: 0.0,
                rule_dedup: 0.15,
                query_max: 0.15
            }
        );
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn empty_whitespace_null_and_missing_values_default() {
        for body in [
            "",
            "  \n\t",
            "null",
            "{}",
            "distances: null",
            "distances: {}",
        ] {
            let root = repo();
            write(&root, body);
            assert_eq!(load(&root).unwrap(), Settings::default());
            let _ = fs::remove_dir_all(root);
        }
    }
    #[test]
    fn malformed_shapes_types_and_ranges_are_configuration_errors() {
        for body in [
            "[1]",
            "distances: [1]",
            "distances:\n  task_dedup: nope",
            "distances:\n  task_dedup: -0.1",
            "distances:\n  task_dedup: 2.1",
            "distances:\n  task_dedup: .NaN",
            "distances:\n  task_dedup: .Inf",
            "distances: {task_dedup:",
        ] {
            let root = repo();
            write(&root, body);
            assert!(matches!(
                load(&root),
                Err(LibraryError::Configuration { .. })
            ));
            let _ = fs::remove_dir_all(root);
        }
    }
    #[test]
    fn boundaries_are_valid_and_missing_file_defaults() {
        let root = repo();
        assert_eq!(load(&root).unwrap(), Settings::default());
        write(
            &root,
            "distances:\n  task_dedup: 0\n  rule_dedup: 2\n  query_max: 1.25\n",
        );
        assert_eq!(load(&root).unwrap().rule_dedup, 2.0);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn unreadable_settings_is_not_treated_as_missing() {
        let root = repo();
        fs::create_dir_all(root.join(".infraware/settings.yaml")).unwrap();
        assert!(matches!(
            load(&root),
            Err(LibraryError::Configuration { .. })
        ));
        let _ = fs::remove_dir_all(root);
    }
}
