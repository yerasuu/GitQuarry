use gitquarry_federation::web::session::*;

use axum::http::HeaderMap;
use gitquarry_wire::Uid;

#[test]
fn escape_html_neutralizes_the_five_reserved_characters() {
    assert_eq!(
        escape_html(r#"<script>alert('x')&"y"</script>"#),
        "&lt;script&gt;alert(&#39;x&#39;)&amp;&quot;y&quot;&lt;/script&gt;"
    );
}

#[test]
fn forwarded_headers_round_trip() {
    let mut headers = HeaderMap::new();
    headers.insert(USER_HEADER, "alice".parse().unwrap());
    headers.insert(USER_UID_HEADER, "abc123".parse().unwrap());
    headers.insert(CSRF_HEADER, "tok".parse().unwrap());
    assert_eq!(forwarded_user(&headers).as_deref(), Some("alice"));
    assert_eq!(forwarded_user_uid(&headers), Some(Uid::from("abc123".to_string())));
    assert_eq!(forwarded_csrf(&headers), "tok");
}

#[test]
fn missing_headers_are_none_or_empty() {
    let headers = HeaderMap::new();
    assert_eq!(forwarded_user(&headers), None);
    assert_eq!(forwarded_user_uid(&headers), None);
    assert_eq!(forwarded_csrf(&headers), "");
}
