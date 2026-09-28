use std::time::{Duration, SystemTime};

use nix::libc;

const SECONDS_PER_DAY: i64 = 86_400;

pub fn parse_rfc3339(text: &str) -> Option<SystemTime> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    if bytes[4] != b'-' || bytes[7] != b'-' || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    if bytes[10] != b'T' && bytes[10] != b't' {
        return None;
    }
    let year = digits(&bytes[0..4])?;
    let month = digits(&bytes[5..7])?;
    let day = digits(&bytes[8..10])?;
    let hour = digits(&bytes[11..13])?;
    let minute = digits(&bytes[14..16])?;
    let second = digits(&bytes[17..19])?;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    let mut rest = &bytes[19..];
    let mut nanos = 0;
    if rest[0] == b'.' {
        let mut count = 0;
        for byte in &rest[1..] {
            if !byte.is_ascii_digit() {
                break;
            }
            count += 1;
        }
        if count == 0 {
            return None;
        }
        let mut scale = 100_000_000;
        for byte in &rest[1..=count] {
            nanos += u32::from(byte - b'0') * scale;
            scale /= 10;
        }
        rest = &rest[count + 1..];
    }

    let offset = parse_offset(rest)?;
    let seconds = days_from_civil(year, month, day) * SECONDS_PER_DAY
        + i64::from(hour) * 3600
        + i64::from(minute) * 60
        + i64::from(second)
        - offset;
    let whole = Duration::from_secs(seconds.unsigned_abs());
    let base = match seconds >= 0 {
        true => SystemTime::UNIX_EPOCH.checked_add(whole)?,
        false => SystemTime::UNIX_EPOCH.checked_sub(whole)?,
    };
    base.checked_add(Duration::from_nanos(u64::from(nanos)))
}

fn parse_offset(bytes: &[u8]) -> Option<i64> {
    if bytes == b"Z" || bytes == b"z" {
        return Some(0);
    }
    if bytes.len() != 6 || bytes[3] != b':' {
        return None;
    }
    let sign = match bytes[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let hours = digits(&bytes[1..3])?;
    let minutes = digits(&bytes[4..6])?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (i64::from(hours) * 3600 + i64::from(minutes) * 60))
}

fn digits(bytes: &[u8]) -> Option<u32> {
    let mut value = 0;
    for byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u32::from(byte - b'0');
    }
    Some(value)
}

fn is_leap_year(year: u32) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn days_from_civil(year: u32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn local_tm(time: SystemTime) -> Option<libc::tm> {
    let seconds = match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_secs() as libc::time_t,
        Err(error) => -(error.duration().as_secs() as libc::time_t),
    };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::localtime_r(&seconds, &mut tm) };
    if result.is_null() {
        return None;
    }
    Some(tm)
}

pub fn format_local_minute(time: SystemTime) -> String {
    let Some(tm) = local_tm(time) else {
        return String::from("-");
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min
    )
}

pub fn format_local_rfc3339(time: SystemTime) -> String {
    let Some(tm) = local_tm(time) else {
        return String::from("-");
    };
    let offset = tm.tm_gmtoff;
    let sign = match offset < 0 {
        true => '-',
        false => '+',
    };
    let offset = offset.unsigned_abs();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        offset / 3600,
        offset % 3600 / 60
    )
}

pub fn unix_seconds(time: SystemTime) -> u64 {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_secs(),
        Err(_) => 0,
    }
}

pub fn from_unix_seconds(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

pub fn format_idle(idle: Duration) -> String {
    let minutes = idle.as_secs() / 60;
    let hours = minutes / 60;
    let minutes = minutes % 60;
    if hours == 0 {
        return format!("{minutes}m");
    }
    format!("{hours}h {minutes}m")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn utc_with_fraction() {
        assert_eq!(
            parse_rfc3339("2026-08-11T22:41:01.578Z"),
            Some(at(1786488061) + Duration::from_millis(578))
        );
    }

    #[test]
    fn utc_without_fraction() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(at(0)));
        assert_eq!(parse_rfc3339("2000-03-01T00:00:00Z"), Some(at(951868800)));
    }

    #[test]
    fn positive_and_negative_offsets() {
        let utc = parse_rfc3339("2026-08-11T20:41:01Z").unwrap();
        assert_eq!(parse_rfc3339("2026-08-11T22:41:01+02:00"), Some(utc));
        assert_eq!(parse_rfc3339("2026-08-11T15:11:01-05:30"), Some(utc));
    }

    #[test]
    fn nanosecond_fraction() {
        assert_eq!(
            parse_rfc3339("1970-01-01T00:00:01.123456789Z"),
            Some(at(1) + Duration::from_nanos(123_456_789))
        );
    }

    #[test]
    fn leap_day() {
        assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z"), Some(at(1709164800)));
        assert_eq!(parse_rfc3339("2026-02-29T00:00:00Z"), None);
    }

    #[test]
    fn invalid_inputs() {
        let inputs = [
            "",
            "2026-08-11",
            "2026-08-11 22:41:01Z",
            "2026-08-11T22:41:01",
            "2026-13-11T22:41:01Z",
            "2026-08-32T22:41:01Z",
            "2026-08-11T24:00:00Z",
            "2026-08-11T22:41:01.Z",
            "2026-08-11T22:41:01+0200",
            "2026-08-11T22:41:01+02:00x",
            "2026-08-11T22:41:01*02:00",
            "20x6-08-11T22:41:01Z",
            "not a timestamp at all",
        ];
        for input in inputs {
            assert_eq!(parse_rfc3339(input), None, "{input}");
        }
    }

    #[test]
    fn local_minute_shape() {
        let text = format_local_minute(at(1790500000));
        let bytes = text.as_bytes();
        assert_eq!(bytes.len(), 16, "{text}");
        assert_eq!(bytes[4], b'-');
        assert_eq!(bytes[7], b'-');
        assert_eq!(bytes[10], b' ');
        assert_eq!(bytes[13], b':');
        assert!(text.starts_with("2026-09-2"), "{text}");
        assert_ne!(format_local_minute(at(1790500000 + 60)), text);
    }

    #[test]
    fn local_rfc3339_round_trips() {
        let time = at(1790500000);
        let text = format_local_rfc3339(time);
        assert_eq!(text.len(), 25, "{text}");
        assert_eq!(parse_rfc3339(&text), Some(time));
    }

    #[test]
    fn unix_seconds_round_trip() {
        assert_eq!(unix_seconds(from_unix_seconds(1790500000)), 1790500000);
        assert_eq!(unix_seconds(at(5) + Duration::from_millis(900)), 5);
        assert_eq!(
            unix_seconds(SystemTime::UNIX_EPOCH - Duration::from_secs(1)),
            0
        );
    }

    #[test]
    fn idle_formats() {
        assert_eq!(format_idle(Duration::ZERO), "0m");
        assert_eq!(format_idle(Duration::from_secs(45 * 60 + 59)), "45m");
        assert_eq!(
            format_idle(Duration::from_secs(3 * 3600 + 12 * 60)),
            "3h 12m"
        );
        assert_eq!(
            format_idle(Duration::from_secs(26 * 3600 + 12 * 60)),
            "26h 12m"
        );
        assert_eq!(format_idle(Duration::from_secs(3600)), "1h 0m");
    }
}
