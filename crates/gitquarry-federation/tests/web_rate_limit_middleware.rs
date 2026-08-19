use gitquarry_federation::web::rate_limit_middleware::*;

use gitquarry_federation::access;
use gitquarry_federation::core_client::CoreClient;
use gitquarry_federation::entities::Peer;
use gitquarry_federation::gossip::GossipTable;
use gitquarry_federation::state::FederationState;
use s3db::{BackendConfig, LocalBackend, LocalEngine, LocalParams};

#[test]
fn url_host_strips_scheme_port_and_path() {
    assert_eq!(url_host("https://acme.example:8080/foo"), "acme.example");
    assert_eq!(url_host("http://127.0.0.1:9418"), "127.0.0.1");
    assert_eq!(url_host("ssh://acme.example"), "acme.example");
}

fn test_state(root: &std::path::Path, peer_override_by_ip: bool) -> FederationState {
    let bucket = root.to_string_lossy().to_string();
    let backend = LocalBackend::new(&LocalParams {
        bucket: bucket.clone(),
        engine: LocalEngine::Files,
    });
    let cipher = s3db_crypto::Cipher::derive("test passphrase", &[71u8; s3db_crypto::SALT_LEN]).unwrap();
    FederationState {
        backend: std::sync::Arc::new(backend),
        cipher: std::sync::Arc::new(cipher),
        core: CoreClient::new("http://127.0.0.1:1".to_string(), "unused".to_string()),
        backend_config: BackendConfig::Local(LocalParams {
            bucket,
            engine: LocalEngine::Files,
        }),
        gossip: std::sync::Arc::new(GossipTable::default()),
        self_announce: None,
        rate_limiter: std::sync::Arc::new(gitquarry_federation::rate_limit::PeerRateLimiter::default()),
        rate_limit_public_max: 1,
        rate_limit_public_window: std::time::Duration::from_secs(60),
        rate_limit_token_max: 120,
        rate_limit_token_window: std::time::Duration::from_secs(60),
        rate_limit_peer_override_by_ip: peer_override_by_ip,
    }
}

async fn seed_boosted_peer(state: &FederationState) {
    let peer = Peer {
        name: "acme".to_string(),
        web_base_url: "https://1.2.3.4".to_string(),
        git_base_url: "https://1.2.3.4".to_string(),
        rate_limit_override: Some(100),
        ..Default::default()
    };
    access::put_peer(state.backend.as_ref(), &state.cipher, &peer, None)
        .await
        .unwrap();
}

#[tokio::test]
async fn peer_override_applies_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path(), true);
    seed_boosted_peer(&state).await;
    // Instance ceiling is 1, but the matching peer's override is 100 —
    // both calls succeed only if the override actually applied.
    assert!(check(&state, None, "1.2.3.4".to_string()).await);
    assert!(check(&state, None, "1.2.3.4".to_string()).await);
}

#[tokio::test]
async fn peer_override_disabled_falls_back_to_the_instance_ceiling() {
    let dir = tempfile::tempdir().unwrap();
    let state = test_state(dir.path(), false);
    seed_boosted_peer(&state).await;
    // Same boosted peer, same IP, but the toggle is off: capped at the
    // instance-wide ceiling (1) regardless.
    assert!(check(&state, None, "1.2.3.4".to_string()).await);
    assert!(!check(&state, None, "1.2.3.4".to_string()).await);
}
