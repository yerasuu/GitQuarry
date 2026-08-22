use gitquarry_webui::webui::repo::{relative_time_ago, relative_time_words};

const MINUTE: u64 = 60;
const HOUR: u64 = 3600;
const DAY: u64 = 86_400;
const WEEK: u64 = 604_800;
const MONTH: u64 = 2_629_800;
const YEAR: u64 = 31_557_600;

/// `now - then` in `words`, at `now = YEAR * 10` so subtracting even the
/// largest bucket here never underflows `then`.
fn words_ago(secs_ago: u64) -> String {
    let now = YEAR * 10;
    relative_time_words(now - secs_ago, now)
}
fn short_ago(secs_ago: u64) -> String {
    let now = YEAR * 10;
    relative_time_ago(now - secs_ago, now)
}

#[test]
fn words_covers_every_bucket_the_mockup_shows() {
    assert_eq!(words_ago(0), "just now");
    assert_eq!(words_ago(59), "just now");
    assert_eq!(words_ago(3 * HOUR), "3 hours ago");
    assert_eq!(words_ago(5 * HOUR), "5 hours ago");
    assert_eq!(words_ago(DAY), "yesterday");
    assert_eq!(words_ago(6 * DAY), "6 days ago");
    assert_eq!(words_ago(3 * WEEK), "3 weeks ago");
    assert_eq!(words_ago(2 * MONTH), "2 months ago");
}

#[test]
fn words_singular_forms_have_no_trailing_s() {
    assert_eq!(words_ago(MINUTE), "1 minute ago");
    assert_eq!(words_ago(HOUR), "1 hour ago");
    assert_eq!(words_ago(WEEK), "1 week ago");
    assert_eq!(words_ago(MONTH), "1 month ago");
    assert_eq!(words_ago(YEAR), "1 year ago");
}

#[test]
fn words_plural_forms_have_a_trailing_s() {
    assert_eq!(words_ago(2 * MINUTE), "2 minutes ago");
    assert_eq!(words_ago(2 * HOUR), "2 hours ago");
    assert_eq!(words_ago(2 * WEEK), "2 weeks ago");
    assert_eq!(words_ago(2 * MONTH), "2 months ago");
    assert_eq!(words_ago(2 * YEAR), "2 years ago");
}

/// `yesterday` is its own bucket, not "1 day ago" — it covers the whole
/// [24h, 48h) span, not just exactly-24h.
#[test]
fn words_yesterday_covers_the_whole_24_to_48_hour_span() {
    assert_eq!(words_ago(DAY), "yesterday");
    assert_eq!(words_ago(DAY + HOUR), "yesterday");
    assert_eq!(words_ago(2 * DAY - 1), "yesterday");
    assert_eq!(words_ago(2 * DAY), "2 days ago");
}

#[test]
fn words_bucket_boundaries_land_on_the_right_side() {
    assert_eq!(words_ago(MINUTE - 1), "just now");
    assert_eq!(words_ago(MINUTE), "1 minute ago");
    assert_eq!(words_ago(HOUR - 1), "59 minutes ago");
    assert_eq!(words_ago(HOUR), "1 hour ago");
    assert_eq!(words_ago(DAY - 1), "23 hours ago");
    assert_eq!(words_ago(WEEK - 1), "6 days ago");
    assert_eq!(words_ago(WEEK), "1 week ago");
    assert_eq!(words_ago(MONTH - 1), "4 weeks ago");
    assert_eq!(words_ago(MONTH), "1 month ago");
    assert_eq!(words_ago(YEAR - 1), "11 months ago");
    assert_eq!(words_ago(YEAR), "1 year ago");
}

#[test]
fn short_covers_every_bucket_with_a_unit_letter() {
    assert_eq!(short_ago(0), "just now");
    assert_eq!(short_ago(3 * HOUR), "3h ago");
    assert_eq!(short_ago(DAY), "1d ago");
    assert_eq!(short_ago(6 * DAY), "6d ago");
    assert_eq!(short_ago(3 * WEEK), "3w ago");
    assert_eq!(short_ago(2 * MONTH), "2mo ago");
    assert_eq!(short_ago(2 * YEAR), "2y ago");
}

#[test]
fn short_bucket_boundaries_land_on_the_right_side() {
    assert_eq!(short_ago(MINUTE - 1), "just now");
    assert_eq!(short_ago(MINUTE), "1m ago");
    assert_eq!(short_ago(HOUR - 1), "59m ago");
    assert_eq!(short_ago(HOUR), "1h ago");
    assert_eq!(short_ago(DAY - 1), "23h ago");
    assert_eq!(short_ago(DAY), "1d ago");
    assert_eq!(short_ago(WEEK - 1), "6d ago");
    assert_eq!(short_ago(WEEK), "1w ago");
    assert_eq!(short_ago(MONTH - 1), "4w ago");
    assert_eq!(short_ago(MONTH), "1mo ago");
    assert_eq!(short_ago(YEAR - 1), "11mo ago");
    assert_eq!(short_ago(YEAR), "1y ago");
}

/// `then > now` (a clock skew, or a stamp written a moment after `now_unix`
/// was captured for this render) must never underflow the `u64` subtraction
/// — `saturating_sub` clamps it to `0`, read as "just now", not a panic.
#[test]
fn a_future_timestamp_reads_as_just_now_instead_of_panicking() {
    assert_eq!(relative_time_words(100, 50), "just now");
    assert_eq!(relative_time_ago(100, 50), "just now");
}
