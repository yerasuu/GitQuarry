use gitquarry_tracker::core_client::CoreClient;
use gitquarry_wire::ipc::LOOPBACK_REQUEST_TIMEOUT;
use std::time::{Duration, Instant};

/// A core that accepts the connection and never answers must fail the call
/// once the loopback timeout passes, instead of holding the request forever.
#[tokio::test]
async fn a_core_that_never_answers_times_out() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    let core = CoreClient::new(format!("http://{addr}"), "unused".to_string()).unwrap();

    let started = Instant::now();
    let result = tokio::time::timeout(
        LOOPBACK_REQUEST_TIMEOUT + Duration::from_secs(10),
        core.repo_labels("uid"),
    )
    .await;
    let error = result.expect("the client's own timeout fires first").unwrap_err();
    assert!(started.elapsed() >= LOOPBACK_REQUEST_TIMEOUT);
    let reqwest_error = error
        .chain()
        .find_map(|e| e.downcast_ref::<reqwest::Error>())
        .expect("a reqwest error in the chain");
    assert!(reqwest_error.is_timeout(), "{error:#}");
}
