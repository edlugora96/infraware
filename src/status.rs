use crate::{
    dispatch::DispatchOutput, error::LibraryError, format::OutputFormat, settings::Settings,
};
use std::path::Path;
fn err() -> LibraryError {
    LibraryError::Io {
        message: "store unavailable".into(),
    }
}
pub(crate) fn run(root: &Path, settings: Settings) -> Result<DispatchOutput, LibraryError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| err())?
        .as_millis() as i64;
    let mut store = match crate::vec_store::SqliteStore::open_read_only(root) {
        Ok(store) => store,
        Err(LibraryError::UninitializedStorage) => return Err(LibraryError::UninitializedStorage),
        Err(_) => return Err(err()),
    };
    let (tasks, rules) = store.status_rows().map_err(|_| err())?;
    let _ = tasks;
    let mut sr = Vec::new();
    for r in rules.iter() {
        if crate::staleness::stale(r, now, settings.staleness_threshold_days).map_err(|_| err())? {
            sr.push(format!(
                "{{\"id\":{},\"text\":{},\"last_revised_at\":{}}}",
                r.id,
                serde_json::to_string(&r.text).unwrap(),
                r.updated_at.unwrap_or(r.created_at)
            ));
        }
    }
    let body = format!(
        "{{\"all_up_to_date\":{},\"window_days\":{},\"stale_rules\":[{}]}}",
        sr.is_empty(),
        settings.staleness_threshold_days,
        sr.join(",")
    );
    Ok(DispatchOutput {
        format: OutputFormat::Json,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    fn root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "infraware-status-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    #[test]
    fn reports_rules_only_with_fixed_shape() {
        let p = root();
        let s = crate::vec_store::SqliteStore::open(&p).unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        s.insert_status_test_row("tasks", "task", now - 15 * 86_400_000_i64, None)
            .unwrap();
        s.insert_status_test_row(
            "rules",
            "rule",
            now - 15 * 86_400_000_i64,
            Some(now - 15 * 86_400_000_i64),
        )
        .unwrap();
        drop(s);
        let out = run(
            &p,
            crate::settings::Settings {
                task_dedup: 0.15,
                rule_dedup: 0.15,
                query_max: 0.15,
                query_auto_ingest: false,
                staleness_threshold_days: 14.0,
            },
        )
        .unwrap();
        assert!(out.body.contains("\"all_up_to_date\":false"));
        assert!(!out.body.contains("stale_tasks"));
        assert!(
            out.body
                .contains("\"stale_rules\":[{\"id\":1,\"text\":\"rule\",\"last_revised_at\":")
        );
        assert!(out.body.contains("\"window_days\":14"));
        let _ = std::fs::remove_dir_all(p);
    }
}
