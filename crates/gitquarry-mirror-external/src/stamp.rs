//! Comparing a local timestamp against an external forge's.
//!
//! Both sides are RFC 3339, but not in the same *shape*: this codebase
//! writes whole seconds and a literal `Z`
//! (`storage::audit::format_unix_secs`), GitLab hands back milliseconds,
//! and a self-hosted Gitea reports its server's own UTC offset. A plain
//! `str` comparison — what Track B's last-write-wins used to do — reads
//! `…:00.000Z` as *earlier* than `…:00Z` (`'.' < 'Z'`) and `+02:00` as
//! later than the same instant in `Z`, so an edit could win or lose on
//! formatting rather than on when it happened.

/// Unix seconds for an RFC 3339 stamp, or `None` if it isn't one this can
/// read — sub-second precision is dropped on purpose: the local side only
/// ever has whole seconds to compare against, so keeping the fraction
/// would just reintroduce a resolution the other side can't match.
pub fn to_unix(stamp: &str) -> Option<i64> {
    let bytes = stamp.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let num = |from: usize, to: usize| stamp.get(from..to)?.parse::<i64>().ok();
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = days_from_civil(year, month as u32, day as u32);
    let utc = days * 86_400 + hour * 3600 + minute * 60 + second;
    Some(utc - offset_seconds(&stamp[19..])?)
}

/// `true` if `a` names a strictly later instant than `b`.
///
/// Falls back to a byte comparison when either side is unparseable — the
/// only stamps that reach this are ones a provider or this codebase wrote,
/// so an unreadable one means a format nobody here produces, and the old
/// behaviour is a better answer than "neither is newer".
pub fn later(a: &str, b: &str) -> bool {
    match (to_unix(a), to_unix(b)) {
        (Some(a), Some(b)) => a > b,
        _ => a > b,
    }
}

/// Whichever of the two names the later instant, `a` on a tie — the
/// cursor-advancing form of [`later`].
pub fn max<'a>(a: &'a str, b: &'a str) -> &'a str {
    if later(b, a) { b } else { a }
}

/// Seconds to subtract to reach UTC: `0` for `Z` (or a bare stamp, which
/// every producer here means as UTC), `±HH:MM`/`±HHMM` otherwise.
fn offset_seconds(tail: &str) -> Option<i64> {
    // Sub-second digits sit before the zone designator and carry no
    // offset of their own.
    let tail = match tail.strip_prefix('.') {
        Some(frac) => &frac[frac.find(|c: char| !c.is_ascii_digit()).unwrap_or(frac.len())..],
        None => tail,
    };
    if tail.is_empty() || tail.eq_ignore_ascii_case("z") {
        return Some(0);
    }
    let sign = match tail.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits: String = tail[1..].chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 4 {
        return None;
    }
    let hours = digits[..2].parse::<i64>().ok()?;
    let minutes = digits[2..].parse::<i64>().ok()?;
    Some(sign * (hours * 3600 + minutes * 60))
}

/// Days since 1970-01-01 for a proleptic-Gregorian date — Howard
/// Hinnant's `days_from_civil`, the exact inverse of
/// `storage::audit`'s own `civil_from_days`.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = i64::from(month) + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
