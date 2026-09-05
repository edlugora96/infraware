use crate::error::LibraryError;
use serde_yaml::Value;
use std::path::Path;
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Settings {
    pub task_dedup: f64,
    pub rule_dedup: f64,
    pub query_max: f64,
    pub staleness_threshold_days: f64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            task_dedup: 0.15,
            rule_dedup: 0.15,
            query_max: 0.15,
            staleness_threshold_days: 14.0,
        }
    }
}
fn err(message: String) -> LibraryError {
    LibraryError::Configuration { message }
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
    let mut out = Settings::default();
    for (key, slot) in [
        ("task_dedup", &mut out.task_dedup),
        ("rule_dedup", &mut out.rule_dedup),
        ("query_max", &mut out.query_max),
    ] {
        let Some(v) = root
            .get("distances")
            .and_then(Value::as_mapping)
            .and_then(|m| m.get(Value::String(key.into())))
        else {
            continue;
        };
        if v.is_null() {
            continue;
        }
        let n = v
            .as_f64()
            .ok_or_else(|| err(format!("{key} must be a number")))?;
        if !n.is_finite() || !(0.0..=2.0).contains(&n) {
            return Err(err(format!("{key} must be finite and in [0, 2]")));
        }
        *slot = n;
    }
    if let Some(v) = root.get("staleness_threshold_days") {
        if !v.is_null() {
            let n = v
                .as_f64()
                .ok_or_else(|| err("staleness_threshold_days must be a number".into()))?;
            if !n.is_finite() || n < 0.0 {
                return Err(err(
                    "staleness_threshold_days must be finite and non-negative".into(),
                ));
            }
            out.staleness_threshold_days = n;
        }
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};
    fn root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "infraware-settings-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    fn put(p: &Path, s: &str) {
        fs::create_dir_all(p.join(".infraware")).unwrap();
        fs::write(p.join(".infraware/settings.yaml"), s).unwrap();
    }
    #[test]
    fn defaults_null_and_independence() {
        let p = root();
        put(
            &p,
            "distances:\n task_dedup: 0.2\nstaleness_threshold_days: null\n",
        );
        let s = load(&p).unwrap();
        assert_eq!(s.task_dedup, 0.2);
        assert_eq!(s.rule_dedup, 0.15);
        assert_eq!(s.staleness_threshold_days, 14.0);
        let _ = fs::remove_dir_all(p);
    }
    #[test]
    fn fractional_and_invalid_values() {
        let p = root();
        put(&p, "staleness_threshold_days: 1.5\n");
        assert_eq!(load(&p).unwrap().staleness_threshold_days, 1.5);
        let _ = fs::remove_dir_all(&p);
        for body in [
            "staleness_threshold_days: -1\n",
            "staleness_threshold_days: nope\n",
            "staleness_threshold_days: .NaN\n",
        ] {
            put(&p, body);
            assert!(matches!(load(&p), Err(LibraryError::Configuration { .. })));
            let _ = fs::remove_dir_all(&p);
        }
    }
}
