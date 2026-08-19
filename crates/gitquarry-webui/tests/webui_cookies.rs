use axum::http::HeaderMap;
use gitquarry_webui::webui::cookies::*;

#[test]
fn get_cookie_finds_the_named_cookie_among_several() {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::COOKIE,
        "a=1; web-session=sealed-value; b=2".parse().unwrap(),
    );
    assert_eq!(get_cookie(&headers, "web-session").as_deref(), Some("sealed-value"));
    assert_eq!(get_cookie(&headers, "missing"), None);
}

#[test]
fn get_cookie_none_without_a_cookie_header() {
    assert_eq!(get_cookie(&HeaderMap::new(), "web-session"), None);
}

#[test]
fn set_cookie_carries_value_and_max_age() {
    let (name, value) = set_cookie(&HeaderMap::new(), "web-session", "abc", 3600);
    assert_eq!(name, axum::http::header::SET_COOKIE);
    assert!(value.contains("web-session=abc"));
    assert!(value.contains("Max-Age=3600"));
    assert!(value.contains("HttpOnly"));
    assert!(value.contains("SameSite=Lax"));
}

/// Plain HTTP must not get `Secure` — the browser would drop the cookie and
/// login would silently never stick, which is why it was originally left
/// off unconditionally.
#[test]
fn set_cookie_omits_secure_on_a_plain_http_request() {
    let (_name, value) = set_cookie(&HeaderMap::new(), "web-session", "abc", 3600);
    assert!(!value.contains("Secure"), "{value}");
}

/// Behind a TLS-terminating proxy it must be there, so the session cookie
/// never travels over plain HTTP.
#[test]
fn set_cookie_adds_secure_behind_an_https_proxy() {
    let mut headers = HeaderMap::new();
    headers.insert("x-forwarded-proto", "https".parse().unwrap());
    let (_name, value) = set_cookie(&headers, "web-session", "abc", 3600);
    assert!(value.contains("Secure"), "{value}");
}

#[test]
fn is_https_reads_the_client_facing_hop_and_ignores_other_values() {
    let mut headers = HeaderMap::new();
    assert!(!is_https(&headers));

    headers.insert("x-forwarded-proto", "http".parse().unwrap());
    assert!(!is_https(&headers));

    // A proxy chain appends; the first entry is the hop that faced the client.
    headers.insert("x-forwarded-proto", "https, http".parse().unwrap());
    assert!(is_https(&headers));

    headers.insert("x-forwarded-proto", "HTTPS".parse().unwrap());
    assert!(is_https(&headers));
}
