use data_encoding::BASE32_NOPAD;
use gitquarry_auth_crypto::totp::*;
use hmac::{Hmac, KeyInit, Mac};
use sha1::Sha1;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const STEP_SECS: u64 = 30;

/// RFC 4226 HOTP written out independently of the crate's own, so a code the
/// crate accepts is checked against the spec rather than against itself.
fn reference_hotp(secret_raw: &[u8], counter: u64) -> u32 {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret_raw).unwrap();
    mac.update(&counter.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let bin = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    bin % 1_000_000
}

/// Current step counter, after waiting out the last seconds of a step so the
/// crate's own clock read lands in the same step as the one the code came
/// from — otherwise a skew test near a boundary shifts by one step.
fn settled_counter() -> u64 {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    if secs % STEP_SECS >= STEP_SECS - 2 {
        std::thread::sleep(Duration::from_secs(STEP_SECS - secs % STEP_SECS));
    }
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() / STEP_SECS
}

fn code_at(secret_b32: &str, counter: u64) -> String {
    let raw = BASE32_NOPAD.decode(secret_b32.as_bytes()).unwrap();
    format!("{:06}", reference_hotp(&raw, counter))
}

#[test]
fn reference_hotp_matches_the_rfc_6238_sha1_test_vector() {
    // RFC 6238 Appendix B: secret "12345678901234567890", T=59s -> counter 1
    // -> 94287082 as 8 digits; the crate emits 6, so the low 6 are compared.
    assert_eq!(reference_hotp(b"12345678901234567890", 1), 94_287_082 % 1_000_000);
}

#[test]
fn generate_secret_is_valid_base32_of_160_bits() {
    let decoded = BASE32_NOPAD.decode(generate_secret().as_bytes()).unwrap();
    assert_eq!(decoded.len(), 20);
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
fn verify_code_accepts_the_current_step_and_rejects_a_wrong_one() {
    let secret = generate_secret();
    let counter = settled_counter();
    let code = code_at(&secret, counter);

    assert!(verify_code(&secret, &code));
    assert_eq!(matching_step(&secret, &code), Some(counter));
    let window: Vec<String> = (counter - 1..=counter + 1).map(|c| code_at(&secret, c)).collect();
    if !window.iter().any(|c| c == "000000") {
        assert!(!verify_code(&secret, "000000"));
    }
}

#[test]
fn verify_code_tolerates_one_step_of_skew_but_not_two() {
    let secret = generate_secret();
    let counter = settled_counter();
    let window: Vec<String> = (counter - 1..=counter + 1).map(|c| code_at(&secret, c)).collect();

    assert_eq!(matching_step(&secret, &window[0]), Some(counter - 1));
    assert_eq!(matching_step(&secret, &window[2]), Some(counter + 1));

    // Only asserted when the out-of-window code doesn't collide with one
    // inside it, so this isn't flaky on a 1-in-a-million HOTP collision.
    for outside in [counter - 2, counter + 2] {
        let code = code_at(&secret, outside);
        if !window.contains(&code) {
            assert!(!verify_code(&secret, &code));
        }
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
    assert!(uri.ends_with("&digits=6&period=30"), "{uri}");
}

#[test]
fn qr_svg_renders_a_scannable_svg_for_a_real_otpauth_uri() {
    let uri = otpauth_uri("gitquarry", "alice", &generate_secret());
    let svg = qr_svg(&uri).unwrap();
    assert!(svg.contains("<svg"), "{svg}");
    assert!(svg.contains("viewBox"), "{svg}");
}
