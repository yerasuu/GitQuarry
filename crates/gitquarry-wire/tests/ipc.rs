use axum::http::{HeaderMap, HeaderValue};
use gitquarry_wire::ipc::{INTERNAL_TOKEN_HEADER, token_matches, wire_version_matches};

fn with_token(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(INTERNAL_TOKEN_HEADER, HeaderValue::from_str(token).unwrap());
    headers
}

#[test]
fn token_matches_only_the_exact_secret() {
    assert!(token_matches(&with_token("s3cret-token"), "s3cret-token"));
    assert!(!token_matches(&with_token("s3cret-tokeN"), "s3cret-token"));
    assert!(!token_matches(&with_token("s3cret"), "s3cret-token"));
    assert!(!token_matches(&with_token("s3cret-token-and-more"), "s3cret-token"));
    assert!(!token_matches(&HeaderMap::new(), "s3cret-token"));
}

#[test]
fn wire_version_matches_requires_the_exact_current_version() {
    let mut headers = HeaderMap::new();
    // Missing header at all: an old caller that predates this check.
    assert!(!wire_version_matches(&headers));
    headers.insert(
        gitquarry_wire::WIRE_VERSION_HEADER,
        (gitquarry_wire::WIRE_PROTOCOL_VERSION + 1).to_string().parse().unwrap(),
    );
    assert!(!wire_version_matches(&headers));
    headers.insert(
        gitquarry_wire::WIRE_VERSION_HEADER,
        gitquarry_wire::WIRE_PROTOCOL_VERSION.to_string().parse().unwrap(),
    );
    assert!(wire_version_matches(&headers));
}
