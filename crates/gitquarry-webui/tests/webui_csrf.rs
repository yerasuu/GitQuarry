use axum::http::HeaderMap;
use gitquarry_webui::webui::csrf::*;

#[test]
fn csrf_field_empty_without_a_session_cookie() {
    assert_eq!(csrf_field(&HeaderMap::new()), "");
}

/// The token in the markup must be *derived* from the session, never the
/// session itself — every authenticated page carries this field, so
/// embedding the cookie would put a live credential in the DOM and undo
/// what `HttpOnly` is there for.
#[test]
fn csrf_field_embeds_a_token_and_never_the_session_cookie_itself() {
    let mut headers = HeaderMap::new();
    headers.insert(axum::http::header::COOKIE, "web-session=sealed-abc".parse().unwrap());
    let field = csrf_field(&headers);
    assert!(field.contains(r#"name="csrf""#));
    assert!(
        !field.contains("sealed-abc"),
        "session cookie leaked into the form: {field}"
    );
}

#[test]
fn require_csrf_accepts_the_token_this_session_renders() {
    let mut headers = HeaderMap::new();
    headers.insert(axum::http::header::COOKIE, "web-session=sealed-abc".parse().unwrap());
    let field = csrf_field(&headers);
    let token = field.split(r#"value=""#).nth(1).unwrap().split('"').next().unwrap();
    assert!(require_csrf(&headers, token).is_ok());
}

/// A token minted under one session must not validate under another.
#[test]
fn require_csrf_rejects_another_sessions_token() {
    let mut mine = HeaderMap::new();
    mine.insert(axum::http::header::COOKIE, "web-session=sealed-abc".parse().unwrap());
    let mut theirs = HeaderMap::new();
    theirs.insert(axum::http::header::COOKIE, "web-session=sealed-xyz".parse().unwrap());
    let their_field = csrf_field(&theirs);
    let their_token = their_field
        .split(r#"value=""#)
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert!(require_csrf(&mine, their_token).is_err());
}

#[test]
fn require_csrf_rejects_a_mismatched_or_missing_token() {
    let mut headers = HeaderMap::new();
    headers.insert(axum::http::header::COOKIE, "web-session=sealed-abc".parse().unwrap());
    assert!(require_csrf(&headers, "wrong").is_err());
    assert!(require_csrf(&headers, "").is_err());
    // The raw cookie value is not the token either.
    assert!(require_csrf(&headers, "sealed-abc").is_err());
    // No session cookie at all: nothing to compare against, always rejected.
    assert!(require_csrf(&HeaderMap::new(), "sealed-abc").is_err());
}

/// `/_login` has no session to anchor on, so it carries its own cookie —
/// `require_csrf_for` is the same double-submit check told which one.
#[test]
fn require_csrf_for_anchors_on_the_named_cookie() {
    let mut headers = HeaderMap::new();
    headers.insert(axum::http::header::COOKIE, "login-csrf=anchor-abc".parse().unwrap());
    let token = csrf_token("anchor-abc");
    assert!(require_csrf_for(&headers, "login-csrf", &token).is_ok());
    // The anchor value itself is not the token.
    assert!(require_csrf_for(&headers, "login-csrf", "anchor-abc").is_err());
    // Nor is a token derived from some other cookie.
    assert!(require_csrf_for(&headers, "login-csrf", &csrf_token("other")).is_err());
    // A missing anchor fails closed rather than accepting anything.
    assert!(require_csrf_for(&HeaderMap::new(), "login-csrf", &token).is_err());
}
