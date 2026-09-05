#[derive(Debug, Clone)]
pub(crate) struct FreshnessRow {
    pub id: i64,
    pub text: String,
    pub created_at: i64,
    pub updated_at: Option<i64>,
}
pub(crate) fn stale(row: &FreshnessRow, now_ms: i64, days: f64) -> Result<bool, String> {
    let ms = days * 86_400_000.0;
    if !days.is_finite() || days < 0.0 || !ms.is_finite() || ms > i64::MAX as f64 {
        return Err(String::from("invalid window"));
    }
    let cutoff = now_ms.checked_sub(ms as i64).ok_or("invalid window")?;
    Ok(row.updated_at.unwrap_or(row.created_at) < cutoff)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn row(t: i64, u: Option<i64>) -> FreshnessRow {
        FreshnessRow {
            id: 1,
            text: "x".into(),
            created_at: t,
            updated_at: u,
        }
    }
    #[test]
    fn fallback_and_strict_cutoff() {
        let now = 1000 + 86_400_000;
        assert!(!stale(&row(1000, None), now, 1.0).unwrap());
        assert!(stale(&row(999, None), now, 1.0).unwrap());
        assert!(!stale(&row(1000, Some(1000)), now, 1.0).unwrap());
    }
    #[test]
    fn rejects_invalid_or_overflowing_windows() {
        assert!(stale(&row(0, None), 0, -1.0).is_err());
        assert!(stale(&row(0, None), 0, f64::INFINITY).is_err());
        assert!(stale(&row(0, None), i64::MIN, 1.0).is_err());
    }
}
