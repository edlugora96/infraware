use chrono::{DateTime, Datelike, FixedOffset, Local, TimeZone};

/// Formats an epoch millisecond timestamp relative to a supplied local date.
pub(crate) fn humanize(timestamp: i64, reference: DateTime<FixedOffset>) -> String {
    let zone = reference.offset().to_owned();
    humanize_in(timestamp, reference, &zone)
}

pub(crate) fn humanize_in<Tz: TimeZone>(
    timestamp: i64,
    reference: DateTime<Tz>,
    timezone: &Tz,
) -> String {
    let Some(value) = timezone.timestamp_millis_opt(timestamp).single() else {
        return timestamp.to_string();
    };
    let date = value.date_naive();
    let today = reference.date_naive();
    let days = (today - date).num_days();
    if days < 0 {
        return "today".into();
    }
    if days == 0 {
        return "today".into();
    }
    if days == 1 {
        return "yesterday".into();
    }
    let this_week = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    let value_week = date - chrono::Duration::days(date.weekday().num_days_from_monday() as i64);
    if value_week == this_week - chrono::Duration::days(7) {
        return "last week".into();
    }
    let month_index = |year: i32, month: u32| year * 12 + month as i32;
    if month_index(today.year(), today.month()) - month_index(date.year(), date.month()) == 1 {
        return "last month".into();
    }
    let months = month_index(today.year(), today.month()) - month_index(date.year(), date.month());
    if date.year() + 1 == today.year() {
        return "last year".into();
    }
    if months >= 12 {
        let years = months / 12;
        return format!("{years} years ago");
    }
    if months > 1 {
        return format!("{months} months ago");
    }
    if days >= 7 && date < this_week {
        let weeks = days / 7;
        return format!("{weeks} {} ago", if weeks == 1 { "week" } else { "weeks" });
    }
    format!("{days} days ago")
}

pub(crate) fn production_reference() -> DateTime<FixedOffset> {
    let now: DateTime<Local> = Local::now();
    now.fixed_offset()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn timestamp(zone: FixedOffset, date: &str, hour: u32) -> i64 {
        zone.from_local_datetime(
            &chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .unwrap()
                .and_hms_opt(hour, 0, 0)
                .unwrap(),
        )
        .single()
        .unwrap()
        .timestamp_millis()
    }

    #[test]
    fn calendar_boundaries_and_period_pluralization_are_deterministic() {
        let zone = FixedOffset::east_opt(0).unwrap();
        let reference = zone.with_ymd_and_hms(2024, 6, 12, 0, 30, 0).unwrap();
        for (date, expected) in [
            ("2024-06-13", "today"),
            ("2024-06-12", "today"),
            ("2024-06-11", "yesterday"),
            ("2024-06-03", "last week"),
            ("2024-05-20", "last month"),
            ("2024-04-01", "2 months ago"),
            ("2023-06-01", "last year"),
            ("2022-01-01", "2 years ago"),
            ("2024-06-10", "2 days ago"),
        ] {
            assert_eq!(humanize(timestamp(zone, date, 0), reference), expected);
        }
        let later_reference = zone.with_ymd_and_hms(2024, 6, 30, 0, 30, 0).unwrap();
        assert_eq!(
            humanize(timestamp(zone, "2024-06-05", 0), later_reference),
            "3 weeks ago"
        );
    }

    #[test]
    fn timezone_date_and_unrepresentable_epoch_are_handled() {
        let zone = FixedOffset::east_opt(5 * 3600 + 30 * 60).unwrap();
        let reference = zone.with_ymd_and_hms(2024, 3, 1, 0, 15, 0).unwrap();
        assert_eq!(
            humanize(timestamp(zone, "2024-02-29", 23), reference),
            "yesterday"
        );
        assert_eq!(humanize(i64::MAX, reference), i64::MAX.to_string());
    }
}
