use data_encoding::BASE32_NOPAD;
use gitquarry_auth_crypto::totp::*;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn generate_secret_is_valid_base32_of_the_expected_length() {
    let secret = generate_secret();
    let decoded = BASE32_NOPAD.decode(secret.as_bytes()).unwrap();
    assert_eq!(decoded.len(), SECRET_BYTES);
}

#[test]
fn generate_recovery_code_is_lowercase_hex_grouped_in_four() {
    let code = generate_recovery_code();
    let groups: Vec<&str> = code.split('-').collect();
    assert_eq!(groups.len(), 4);
    for group in groups {
        assert_eq!(group.len(), 4);
        assert!(group.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }
}

#[test]
fn generate_recovery_code_is_not_obviously_repeated() {
    // Not a rigorous randomness test -- just a sanity check that two calls
    // don't return the same code (would indicate a broken RNG source).
    assert_ne!(generate_recovery_code(), generate_recovery_code());
}

#[test]
fn matches_the_rfc_6238_sha1_test_vector() {
    // RFC 6238 Appendix B: 20-byte secret "12345678901234567890" (ASCII),
    // T=59s -> HOTP counter 1 (30s step) -> expected code 94287082
    // (RFC 6238 defines 8-digit codes for its test vectors; this
    // implementation always emits 6, so the low 6 digits are what's
    // checked here against `hotp` directly rather than through
    // `verify_code_at`, which only accepts 6-digit input).
    let secret_raw = b"12345678901234567890";
    let code = hotp(secret_raw, 1).unwrap();
    assert_eq!(code, 94_287_082 % 1_000_000);
}

#[test]
fn verify_code_at_accepts_the_current_step_and_rejects_a_wrong_one() {
    let secret = generate_secret();
    let now = SystemTime::now();
    let secret_raw = BASE32_NOPAD.decode(secret.as_bytes()).unwrap();
    let elapsed = now.duration_since(UNIX_EPOCH).unwrap();
    let counter = elapsed.as_secs() / STEP_SECS;
    let code = hotp(&secret_raw, counter).unwrap();
    let code_str = format!("{code:06}");

    assert!(verify_code_at(&secret, &code_str, now));
    if code_str != "000000" {
        assert!(!verify_code_at(&secret, "000000", now));
    }
}

#[test]
fn verify_code_at_tolerates_one_step_of_skew_but_not_two() {
    let secret = generate_secret();
    let secret_raw = BASE32_NOPAD.decode(secret.as_bytes()).unwrap();
    let now = SystemTime::now();
    let elapsed = now.duration_since(UNIX_EPOCH).unwrap();
    let counter = elapsed.as_secs() / STEP_SECS;

    let one_step_ago = hotp(&secret_raw, counter - 1).unwrap();
    assert!(verify_code_at(&secret, &format!("{one_step_ago:06}"), now));

    let two_steps_ago = hotp(&secret_raw, counter.saturating_sub(2)).unwrap();
    // Only assert rejection when the two-steps-ago code actually differs
    // from anything within the accepted window (extremely unlikely
    // collision aside), so this isn't flaky near a HOTP collision.
    let accepted: Vec<u32> = (-1..=1)
        .map(|skew| hotp(&secret_raw, counter.checked_add_signed(skew).unwrap()).unwrap())
        .collect();
    if !accepted.contains(&two_steps_ago) {
        assert!(!verify_code_at(&secret, &format!("{two_steps_ago:06}"), now));
    }
}

#[test]
fn verify_code_rejects_malformed_input_without_panicking() {
    assert!(!verify_code("not-base32!!", "123456"));
    assert!(!verify_code(&generate_secret(), "12345"));
    assert!(!verify_code(&generate_secret(), "abcdef"));
    assert!(!verify_code(&generate_secret(), ""));
}

#[test]
fn otpauth_uri_percent_encodes_special_characters() {
    let uri = otpauth_uri("git quarry", "alice@example.com", "SECRET");
    assert!(uri.starts_with("otpauth://totp/git%20quarry:alice%40example.com?secret=SECRET"));
}

#[test]
fn qr_svg_renders_a_scannable_svg_for_a_real_otpauth_uri() {
    let uri = otpauth_uri("gitquarry", "alice", &generate_secret());
    let svg = qr_svg(&uri).unwrap();
    assert!(svg.contains("<svg"), "{svg}");
    assert!(svg.contains("viewBox"), "{svg}");
}

#[test]
fn verify_code_at_is_stable_across_the_unix_epoch_boundary() {
    // Sanity check that duration_since never panics for a `now` at/after
    // the epoch (the only case this function has to handle).
    assert!(!verify_code_at(
        "AAAAAAAAAAAAAAAA",
        "000000",
        UNIX_EPOCH + Duration::from_secs(0)
    ));
}
