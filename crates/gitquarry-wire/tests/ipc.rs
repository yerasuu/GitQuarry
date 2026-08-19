use axum::http::HeaderMap;
use gitquarry_wire::ipc::{INTERNAL_TOKEN_HEADER, token_matches, wire_version_matches};

#[test]
fn token_matches_requires_an_exact_header_match() {
    let mut headers = HeaderMap::new();
    assert!(!token_matches(&headers, "secret"));
    headers.insert(INTERNAL_TOKEN_HEADER, "wrong".parse().unwrap());
    assert!(!token_matches(&headers, "secret"));
    headers.insert(INTERNAL_TOKEN_HEADER, "secret".parse().unwrap());
    assert!(token_matches(&headers, "secret"));
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
