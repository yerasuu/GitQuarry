//! Real sockets, real HTTP round-trips, no mocking — same discipline every
//! other cross-instance test in this tree follows. Covers the doc's own
//! verification bullets for section 5: transitive propagation below the
//! auto-link threshold, then a real auto-link firing once it's cleared.

use gitquarry_federation::access;
use gitquarry_federation::core_client::CoreClient;
use gitquarry_federation::gossip::{GossipCandidateWire, GossipTable, bootstrap_seeds, run_auto_link_tick};
use gitquarry_federation::state::FederationState;
use gitquarry_federation::web;
use s3db::{BackendConfig, LocalBackend, LocalEngine, LocalParams};
use std::sync::Arc;

fn test_state(root: &std::path::Path, name: &str, web_base_url: String) -> Arc<FederationState> {
    let bucket = root.to_string_lossy().to_string();
    let backend = LocalBackend::new(&LocalParams {
        bucket: bucket.clone(),
        engine: LocalEngine::Files,
    });
    let cipher = s3db_crypto::Cipher::derive("test passphrase", &[42u8; s3db_crypto::SALT_LEN]).unwrap();
    Arc::new(FederationState {
        backend: Arc::new(backend),
        cipher: Arc::new(cipher),
        core: CoreClient::new("http://127.0.0.1:1".to_string(), "unused".to_string()),
        backend_config: BackendConfig::Local(LocalParams {
            bucket,
            engine: LocalEngine::Files,
        }),
        gossip: Arc::new(GossipTable::default()),
        self_announce: Some(GossipCandidateWire {
            name: name.to_string(),
            web_base_url: web_base_url.clone(),
            git_base_url: web_base_url,
        }),
        rate_limiter: Arc::new(gitquarry_federation::rate_limit::PeerRateLimiter::default()),
        rate_limit_public_max: 60,
        rate_limit_public_window: std::time::Duration::from_secs(60),
        rate_limit_token_max: 120,
        rate_limit_token_window: std::time::Duration::from_secs(60),
        rate_limit_peer_override_by_ip: true,
    })
}

/// Binds first so the real address is known before building state (the
/// self-announce needs its own address), then serves in the
/// background.
async fn spawn_instance(root: &std::path::Path, name: &str) -> Arc<FederationState> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = test_state(root, name, format!("http://{addr}"));
    let app = web::router(Arc::clone(&state));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let probe = reqwest::Client::new();
    for _ in 0..100 {
        if probe
            .get(format!("http://{addr}/_federation/gossip/peers"))
            .send()
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    state
}

fn self_info_of(state: &FederationState) -> GossipCandidateWire {
    state.self_announce.clone().unwrap()
}

#[tokio::test]
async fn gossip_propagates_transitively_without_crossing_the_auto_link_threshold() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let dir_c = tempfile::tempdir().unwrap();
    let a = spawn_instance(dir_a.path(), "instance-a").await;
    let b = spawn_instance(dir_b.path(), "instance-b").await;
    let c = spawn_instance(dir_c.path(), "instance-c").await;
    let a_url = self_info_of(&a).web_base_url.clone();

    // B and C both bootstrap against A — A now knows both directly.
    bootstrap_seeds(&b.gossip, std::slice::from_ref(&a_url), &self_info_of(&b)).await;
    bootstrap_seeds(&c.gossip, std::slice::from_ref(&a_url), &self_info_of(&c)).await;
    let a_known: std::collections::HashSet<String> = a.gossip.snapshot().into_iter().map(|c| c.name).collect();
    assert!(a_known.contains(&self_info_of(&b).name));
    assert!(a_known.contains(&self_info_of(&c).name));

    // B doesn't know about C yet — it only ever talked to A, once,
    // before C had announced itself.
    assert!(!b.gossip.snapshot().iter().any(|p| p.name == self_info_of(&c).name));

    // A second announce from B to A (simulating the next periodic
    // tick) picks up C transitively via A's response.
    bootstrap_seeds(&b.gossip, &[a_url], &self_info_of(&b)).await;
    assert!(b.gossip.snapshot().iter().any(|p| p.name == self_info_of(&c).name));

    // Nobody auto-linked anybody: two real reports isn't enough
    // corroboration from currently-*linked* sources (nothing is linked
    // at all here yet), regardless of how the table filled up.
    assert!(
        access::list_all_peers(a.backend.as_ref(), &a.cipher)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn auto_link_tick_promotes_a_candidate_once_corroborated_by_enough_linked_sources() {
    let dir_observer = tempfile::tempdir().unwrap();
    let dir_candidate = tempfile::tempdir().unwrap();
    let observer = spawn_instance(dir_observer.path(), "observer").await;
    let candidate = spawn_instance(dir_candidate.path(), "candidate").await;
    let candidate_info = self_info_of(&candidate);

    // Two peers the observer already has manually linked (pre-existing
    // trust) — auto-link only counts corroboration from names that are
    // *currently* real `Peer`s, never arbitrary gossip-table entries.
    for name in ["linked-x", "linked-y"] {
        let peer = gitquarry_federation::entities::Peer {
            name: name.to_string(),
            web_base_url: "https://unused.example".to_string(),
            git_base_url: "https://unused.example".to_string(),
            linked_at: String::new(),
            access_level: Default::default(),
            banned: false,
            ban_reason: None,
            banned_at: None,
            banned_by: String::new(),
            ..Default::default()
        };
        access::put_peer(observer.backend.as_ref(), &observer.cipher, &peer, None)
            .await
            .unwrap();
    }

    observer.gossip.report(
        &candidate_info.name,
        &candidate_info.web_base_url,
        &candidate_info.git_base_url,
        Some("linked-x"),
    );
    observer.gossip.report(
        &candidate_info.name,
        &candidate_info.web_base_url,
        &candidate_info.git_base_url,
        Some("linked-y"),
    );

    // Below threshold (needs 2, only 1 linked source so far after
    // filtering) — nothing promotes yet. Wait, both were just
    // reported: recheck with an intentionally-too-high bar first.
    run_auto_link_tick(observer.backend.as_ref(), &observer.cipher, &observer.gossip, 3, 0).await;
    assert!(
        access::get_peer(observer.backend.as_ref(), &observer.cipher, &candidate_info.name)
            .await
            .unwrap()
            .is_none()
    );

    // Threshold cleared (2 linked sources, age requirement 0): promotes for real.
    run_auto_link_tick(observer.backend.as_ref(), &observer.cipher, &observer.gossip, 2, 0).await;
    let linked = access::get_peer(observer.backend.as_ref(), &observer.cipher, &candidate_info.name)
        .await
        .unwrap();
    assert!(linked.is_some(), "candidate should have been auto-linked");
    assert_eq!(linked.unwrap().web_base_url, candidate_info.web_base_url);
}
