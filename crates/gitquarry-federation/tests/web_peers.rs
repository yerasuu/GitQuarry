use gitquarry_federation::web::peers::*;

use gitquarry_federation::entities::Peer;

#[test]
fn render_page_lists_each_peer_with_a_delete_button() {
    let peer = Peer {
        name: "acme-instance".to_string(),
        web_base_url: "https://acme.example:8080".to_string(),
        git_base_url: "ssh://acme.example:2222".to_string(),
        linked_at: "2026-08-05T00:00:00Z".to_string(),
        access_level: Default::default(),
        banned: false,
        ban_reason: None,
        banned_at: None,
        banned_by: String::new(),
        ..Default::default()
    };
    let html = render_page(&[peer], "tok", None);
    assert!(html.contains("acme-instance"));
    assert!(html.contains(r#"value="delete_peer""#));
}

#[test]
fn render_page_shows_empty_hint_without_peers() {
    let html = render_page(&[], "tok", None);
    assert!(html.contains("no linked instances yet"));
}

#[test]
fn render_page_escapes_a_hostile_peer_name() {
    let peer = Peer {
        name: "<script>evil</script>".to_string(),
        web_base_url: "https://acme.example".to_string(),
        git_base_url: "https://acme.example".to_string(),
        linked_at: String::new(),
        access_level: Default::default(),
        banned: false,
        ban_reason: None,
        banned_at: None,
        banned_by: String::new(),
        ..Default::default()
    };
    let html = render_page(&[peer], "tok", None);
    assert!(!html.contains("<script>evil</script>"));
    assert!(html.contains("&lt;script&gt;"));
}
