use gitquarry_core::storage::cas_retry_backoff;
use std::time::{Duration, Instant};

#[tokio::test]
async fn backoff_grows_with_attempt_number_and_stays_bounded() {
    let start = Instant::now();
    cas_retry_backoff(0).await;
    let short = start.elapsed();

    let start = Instant::now();
    cas_retry_backoff(4).await;
    let long = start.elapsed();
    assert!(long > short);

    // Even a way-past-any-real-retry-cap attempt number stays capped, not
    // an unbounded exponential blowup.
    let start = Instant::now();
    cas_retry_backoff(50).await;
    assert!(start.elapsed() < Duration::from_secs(1));
}
