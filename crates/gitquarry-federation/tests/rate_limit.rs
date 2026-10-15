use gitquarry_federation::rate_limit::*;
use std::time::Duration;

#[test]
fn allows_up_to_max_then_blocks_within_the_window() {
    let limiter = PeerRateLimiter::default();
    let window = Duration::from_secs(60);
    assert!(limiter.check_public("1.2.3.4", 2, window));
    assert!(limiter.check_public("1.2.3.4", 2, window));
    assert!(!limiter.check_public("1.2.3.4", 2, window));
}

#[test]
fn keys_are_independent() {
    let limiter = PeerRateLimiter::default();
    let window = Duration::from_secs(60);
    assert!(limiter.check_public("1.2.3.4", 1, window));
    assert!(!limiter.check_public("1.2.3.4", 1, window));
    // A different IP has its own independent allowance.
    assert!(limiter.check_public("5.6.7.8", 1, window));
}

#[test]
fn public_and_token_buckets_are_independent() {
    let limiter = PeerRateLimiter::default();
    let window = Duration::from_secs(60);
    assert!(limiter.check_public("alice", 1, window));
    assert!(!limiter.check_public("alice", 1, window));
    // Same key string, but the token bucket is a separate counter.
    assert!(limiter.check_token("alice", 1, window));
}

#[test]
fn a_zero_max_blocks_immediately() {
    let limiter = PeerRateLimiter::default();
    assert!(!limiter.check_public("1.2.3.4", 0, Duration::from_secs(60)));
}

#[test]
fn evicting_idle_keys_never_resets_a_key_still_at_its_ceiling() {
    let limiter = PeerRateLimiter::default();
    let window = Duration::from_millis(200);
    assert!(limiter.check_public("idle", 1, window));
    std::thread::sleep(Duration::from_millis(120));
    assert!(limiter.check_public("busy", 1, window));
    std::thread::sleep(Duration::from_millis(100));
    // Past one window since the first sweep, so this call sweeps: "idle"
    // has expired and goes, "busy" is still inside its window and stays.
    assert!(limiter.check_public("other", 1, window));
    assert!(!limiter.check_public("busy", 1, window));
    assert!(limiter.check_public("idle", 1, window));
}

#[test]
fn an_expired_window_frees_up_the_allowance_again() {
    let limiter = PeerRateLimiter::default();
    let window = Duration::from_millis(20);
    assert!(limiter.check_public("1.2.3.4", 1, window));
    assert!(!limiter.check_public("1.2.3.4", 1, window));
    std::thread::sleep(Duration::from_millis(30));
    assert!(limiter.check_public("1.2.3.4", 1, window));
}
