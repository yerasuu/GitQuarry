use gitquarry_federation::access;
use gitquarry_federation::entities::{AccessToken, Peer};
use gitquarry_federation::web::tokens::*;
use gitquarry_wire::Uid;

fn base_token(write: bool) -> AccessToken {
    AccessToken {
        uid: Uid::new(),
        owner: "alice".to_string(),
        label: "peer A".to_string(),
        token_hash: "deadbeef".to_string(),
        repos: [Uid::new()].into_iter().collect(),
        write,
        created_at: "2026-08-05T00:00:00Z".to_string(),
        expires_at: None,
        last_used_at: None,
    }
}

fn data(
    tokens: Vec<AccessToken>,
    peers: Vec<Peer>,
    peer_credentials: Vec<(String, access::PeerCredential)>,
) -> PageData {
    PageData {
        tokens,
        peers,
        peer_credentials,
    }
}

#[test]
fn empty_state_shows_hints() {
    let html = render_page(&data(vec![], vec![], vec![]), "tok", None, None);
    assert!(html.contains("no tokens yet"));
    assert!(html.contains("no peers linked"));
}

#[test]
fn read_only_and_read_write_tokens_render_distinct_scope_labels() {
    let html = render_page(
        &data(vec![base_token(false), base_token(true)], vec![], vec![]),
        "tok",
        None,
        None,
    );
    assert!(html.contains("read-only"));
    assert!(html.contains("read+write"));
}

#[test]
fn pending_secret_is_shown_once() {
    let html = render_page(&data(vec![], vec![], vec![]), "tok", None, Some("token_abc123"));
    assert!(html.contains("token_abc123"));
    assert!(html.contains("your new token"));
}

#[test]
fn a_linked_peer_offers_the_link_form() {
    let peer = Peer {
        name: "acme".to_string(),
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
    let html = render_page(&data(vec![], vec![peer], vec![]), "tok", None, None);
    assert!(html.contains(r#"value="link_peer""#));
    assert!(html.contains(r#"<option value="acme">acme</option>"#));
}
