//! RFC 3339 timestamps, as the agents write them, without a date crate.

use std::time::{Duration, SystemTime};

/// Parses `2025-09-10T12:34:56Z`, with optional fractional seconds and a
/// `Z` or `±HH:MM` offset. Times before 1970 are not supported.
pub fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    let b = s.trim().as_bytes();
    let num = |range: std::ops::Range<usize>| -> Option<i64> {
        let digits = b.get(range)?;
        if !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        Some(digits.iter().fold(0, |n, d| n * 10 + i64::from(d - b'0')))
    };
    let sep = |i: usize, allowed: &[u8]| b.get(i).is_some_and(|c| allowed.contains(c));
    if !(sep(4, b"-") && sep(7, b"-") && sep(10, b"Tt ") && sep(13, b":") && sep(16, b":")) {
        return None;
    }
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    // Leap seconds (60) are accepted and roll into the next minute.
    if second > 60 {
        return None;
    }

    let mut i = 19;
    let mut nanos = 0u32;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            if i - start < 9 {
                nanos = nanos * 10 + u32::from(b[i] - b'0');
            }
            i += 1;
        }
        let digits = i - start;
        if digits == 0 {
            return None;
        }
        nanos *= 10u32.pow(9u32.saturating_sub(digits as u32));
    }

    let offset = match b.get(i)? {
        b'Z' | b'z' if i + 1 == b.len() => 0,
        sign @ (b'+' | b'-') if i + 6 == b.len() && b[i + 3] == b':' => {
            let minutes = num(i + 1..i + 3)? * 60 + num(i + 4..i + 6)?;
            if *sign == b'+' {
                minutes * 60
            } else {
                -minutes * 60
            }
        }
        _ => return None,
    };

    let secs =
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset;
    let secs = u64::try_from(secs).ok()?;
    Some(SystemTime::UNIX_EPOCH + Duration::new(secs, nanos))
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn utc() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(at(0)));
        // date -u -d 2025-09-10T12:34:56Z +%s
        assert_eq!(
            parse_rfc3339("2025-09-10T12:34:56Z"),
            Some(at(1_757_507_696))
        );
        // A leap day.
        assert_eq!(
            parse_rfc3339("2024-02-29T00:00:00Z"),
            Some(at(1_709_164_800))
        );
    }

    #[test]
    fn fractions() {
        let t = parse_rfc3339("2025-09-10T12:34:56.789Z").unwrap();
        assert_eq!(t, at(1_757_507_696) + Duration::from_millis(789));
        let t = parse_rfc3339("2025-09-10T12:34:56.123456789123Z").unwrap();
        assert_eq!(t, at(1_757_507_696) + Duration::from_nanos(123_456_789));
    }

    #[test]
    fn offsets() {
        let utc = parse_rfc3339("2025-09-10T12:34:56Z");
        assert_eq!(parse_rfc3339("2025-09-10T14:34:56+02:00"), utc);
        assert_eq!(parse_rfc3339("2025-09-10T07:04:56-05:30"), utc);
        assert_eq!(parse_rfc3339("2025-09-10 12:34:56.000+00:00"), utc);
    }

    #[test]
    fn rejects_garbage() {
        for s in [
            "",
            "2025-09-10",
            "2025-09-10T12:34:56",
            "2025-13-10T12:34:56Z",
            "2025-09-10T24:00:00Z",
            "2025-09-10T12:34:56.Z",
            "2025-09-10T12:34:56Zjunk",
            "2025-09-10T12:34:56+0200",
            "2025/09/10T12:34:56Z",
            "1969-12-31T23:59:59Z",
        ] {
            assert_eq!(parse_rfc3339(s), None, "{s}");
        }
    }
}
