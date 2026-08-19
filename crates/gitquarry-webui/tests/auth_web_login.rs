use gitquarry_auth_crypto::password;
use gitquarry_webui::auth::web::login::*;

#[test]
fn safe_return_to_allows_local_paths() {
    assert_eq!(safe_return_to("/team/proj"), "/team/proj");
    assert_eq!(safe_return_to("/team/proj?view=log"), "/team/proj?view=log");
}

#[test]
fn safe_return_to_blocks_open_redirects() {
    // No leading slash at all.
    assert_eq!(safe_return_to("https://evil.com"), "/");
    assert_eq!(safe_return_to("evil.com"), "/");
    // Protocol-relative: browsers resolve `//evil.com` as `https://evil.com`.
    assert_eq!(safe_return_to("//evil.com"), "/");
    // WHATWG URL parsing normalizes `\` to `/` for http(s), so this
    // would otherwise resolve to `//evil.com` too.
    assert_eq!(safe_return_to("/\\evil.com"), "/");
    assert_eq!(safe_return_to(""), "/");
}

#[test]
fn flow_and_session_cookies_roundtrip_through_serde() {
    let flow = FlowCookie {
        csrf: "csrf-value".to_string(),
        nonce: "nonce-value".to_string(),
        pkce_verifier: "verifier-value".to_string(),
        return_to: "/team/proj".to_string(),
        exp: 12345,
    };
    let json = serde_json::to_vec(&flow).unwrap();
    let back: FlowCookie = serde_json::from_slice(&json).unwrap();
    assert_eq!(back.csrf, "csrf-value");
    assert_eq!(back.return_to, "/team/proj");

    let session = SessionCookie {
        subject: SessionSubject::Email("alice@example.com".to_string()),
        exp: 12345,
        epoch: 7,
    };
    let json = serde_json::to_vec(&session).unwrap();
    let back: SessionCookie = serde_json::from_slice(&json).unwrap();
    assert!(matches!(back.subject, SessionSubject::Email(e) if e == "alice@example.com"));
    assert_eq!(back.exp, 12345);
    assert_eq!(back.epoch, 7);
}

#[test]
fn pending_totp_cookie_roundtrips_through_serde() {
    let pending = PendingTotpCookie {
        username: "alice".to_string(),
        return_to: "/team/proj".to_string(),
        exp: 12345,
    };
    let json = serde_json::to_vec(&pending).unwrap();
    let back: PendingTotpCookie = serde_json::from_slice(&json).unwrap();
    assert_eq!(back.username, "alice");
    assert_eq!(back.return_to, "/team/proj");
}

#[test]
fn username_session_roundtrips_through_serde() {
    let session = SessionCookie {
        subject: SessionSubject::Username("admin".to_string()),
        exp: 12345,
        epoch: 0,
    };
    let json = serde_json::to_vec(&session).unwrap();
    let back: SessionCookie = serde_json::from_slice(&json).unwrap();
    assert!(matches!(back.subject, SessionSubject::Username(u) if u == "admin"));
    assert_eq!(back.exp, 12345);
}

#[test]
fn dummy_hash_is_a_valid_argon2_hash_nothing_matches() {
    assert!(!password::verify_password("anything", &DUMMY_HASH));
    assert!(!password::verify_password("", &DUMMY_HASH));
}

/// A cookie sealed before `epoch` existed must still deserialize, as `0` —
/// that is what keeps live sessions valid across the upgrade that added
/// the field, since stored `User` records read as `0` too.
#[test]
fn a_session_cookie_without_an_epoch_reads_as_zero() {
    let legacy = br#"{"subject":{"Username":"admin"},"exp":12345}"#;
    let back: SessionCookie = serde_json::from_slice(legacy).unwrap();
    assert_eq!(back.epoch, 0);
}

/// The short-lived flow cookies carry their own deadline, so a client that
/// simply keeps sending the sealed blob past its `Max-Age` doesn't get an
/// indefinitely replayable login step.
#[test]
fn flow_and_pending_totp_cookies_carry_their_own_expiry() {
    let flow = FlowCookie {
        csrf: "c".to_string(),
        nonce: "n".to_string(),
        pkce_verifier: "v".to_string(),
        return_to: "/".to_string(),
        exp: 999,
    };
    let back: FlowCookie = serde_json::from_slice(&serde_json::to_vec(&flow).unwrap()).unwrap();
    assert_eq!(back.exp, 999);

    let pending = PendingTotpCookie {
        username: "alice".to_string(),
        return_to: "/".to_string(),
        exp: 42,
    };
    let back: PendingTotpCookie = serde_json::from_slice(&serde_json::to_vec(&pending).unwrap()).unwrap();
    assert_eq!(back.exp, 42);
}

/// A cookie sealed before `exp` existed reads as `0` — already past, so it
/// fails closed rather than being honoured forever.
#[test]
fn a_flow_cookie_without_an_expiry_reads_as_already_expired() {
    let legacy = br#"{"csrf":"c","nonce":"n","pkce_verifier":"v","return_to":"/"}"#;
    let back: FlowCookie = serde_json::from_slice(legacy).unwrap();
    assert_eq!(back.exp, 0);
}
