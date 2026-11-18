//! Sizes and dates the way a person reads them.
//!
//! Sizes are decimal — a kilobyte is a thousand bytes — as disks are sold
//! and as GNOME and macOS count, with one decimal below ten and none above:
//! `74 B`, `1.2 kB`, `12 kB`, `403 GB`.
//!
//! Dates say as little as tells them apart: the time for today,
//! `yesterday`, a weekday within the week, the day and month within the
//! year, and the full date beyond.

use jiff::{Timestamp, Zoned, civil::Date, tz::TimeZone};

use crate::list::Time;

/// `bytes` as a short size.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["kB", "MB", "GB", "TB", "PB", "EB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    value /= 1000.0;
    while value >= 999.95 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if value < 9.95 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{:.0} {}", value.round(), UNITS[unit])
    }
}

/// How many things, in words: `1 item`, `12 items`.
pub fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// `time` as a short date, relative to `now` and in `now`'s time zone.
pub fn when(time: Time, now: &Zoned) -> String {
    let Ok(stamp) = Timestamp::from_nanosecond(i128::from(time)) else {
        return String::new();
    };
    let then = stamp.to_zoned(now.time_zone().clone());
    let (day, today) = (then.date(), now.date());
    let ago = now.timestamp().as_second() - stamp.as_second();
    if (0..60).contains(&ago) {
        return "just now".to_owned();
    }
    if (60..3600).contains(&ago) {
        return format!("{} min ago", ago / 60);
    }
    if day == today {
        return then.strftime("%H:%M").to_string();
    }
    if Some(day) == today.yesterday().ok() {
        return "yesterday".to_owned();
    }
    let days = days_between(day, today);
    if (2..7).contains(&days) {
        return then.strftime("%a").to_string();
    }
    if day.year() == today.year() && day < today {
        return then.strftime("%b %-d").to_string();
    }
    then.strftime("%Y-%m-%d").to_string()
}

/// `time` in full, for a tooltip or the properties of a file.
pub fn when_exactly(time: Time, zone: &TimeZone) -> String {
    Timestamp::from_nanosecond(i128::from(time))
        .map(|stamp| {
            stamp
                .to_zoned(zone.clone())
                .strftime("%a %-d %b %Y, %H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}

fn days_between(from: Date, to: Date) -> i64 {
    from.until(to).map_or(0, |span| i64::from(span.get_days()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_a_disk_label() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(74), "74 B");
        assert_eq!(size(999), "999 B");
        assert_eq!(size(1000), "1.0 kB");
        assert_eq!(size(1234), "1.2 kB");
        assert_eq!(size(9_960), "10 kB");
        assert_eq!(size(12_345), "12 kB");
        assert_eq!(size(999_999), "1.0 MB");
        assert_eq!(size(8_400_000), "8.4 MB");
        assert_eq!(size(403_000_000_000), "403 GB");
        assert_eq!(size(u64::MAX), "18 EB");
    }

    #[test]
    fn counts_have_their_plural() {
        assert_eq!(count(0, "item", "items"), "0 items");
        assert_eq!(count(1, "item", "items"), "1 item");
        assert_eq!(count(12, "item", "items"), "12 items");
    }

    fn at(text: &str) -> Zoned {
        format!("{text}[UTC]").parse().unwrap()
    }

    fn nanos(text: &str) -> Time {
        at(text).timestamp().as_nanosecond() as Time
    }

    #[test]
    fn dates_say_as_little_as_tells_them_apart() {
        // A Wednesday.
        let now = at("2026-11-18T15:30:00");
        assert_eq!(when(nanos("2026-11-18T15:29:30"), &now), "just now");
        assert_eq!(when(nanos("2026-11-18T15:10:00"), &now), "20 min ago");
        assert_eq!(when(nanos("2026-11-18T09:05:00"), &now), "09:05");
        assert_eq!(when(nanos("2026-11-17T23:00:00"), &now), "yesterday");
        assert_eq!(when(nanos("2026-11-16T10:00:00"), &now), "Mon");
        assert_eq!(when(nanos("2026-11-12T10:00:00"), &now), "Thu");
        assert_eq!(when(nanos("2026-11-02T10:00:00"), &now), "Nov 2");
        assert_eq!(when(nanos("2025-12-25T10:00:00"), &now), "2025-12-25");
        // From the future (a clock that was wrong): the date, not "ago".
        assert_eq!(when(nanos("2027-01-01T10:00:00"), &now), "2027-01-01");
    }

    #[test]
    fn exact_dates_are_complete() {
        assert_eq!(
            when_exactly(nanos("2026-11-02T10:04:05"), &TimeZone::UTC),
            "Mon 2 Nov 2026, 10:04:05"
        );
    }
}
