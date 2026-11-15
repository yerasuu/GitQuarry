use gitquarry_federation::core_client::CoreClient;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Answers one request with `404` and returns its request line.
async fn request_line_of(call: impl AsyncFnOnce(CoreClient)) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/_internal", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        let n = socket.read(&mut buf).await.unwrap();
        socket
            .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
            .await
            .unwrap();
        String::from_utf8_lossy(&buf[..n]).lines().next().unwrap().to_string()
    });
    call(CoreClient::new(base, "secret".to_string()).unwrap()).await;
    server.await.unwrap()
}

#[tokio::test]
async fn query_values_cannot_add_a_second_parameter() {
    let line = request_line_of(async |core| {
        let _ = core.uid_for_name("a&uid=evil#x y").await;
    })
    .await;
    assert_eq!(
        line,
        "GET /_internal/repos/uid-for-name?name=a%26uid%3Devil%23x+y HTTP/1.1"
    );
}

#[tokio::test]
async fn repo_name_in_a_path_is_percent_encoded() {
    let line = request_line_of(async |core| {
        let _ = core.public_repo("alice/pro j?x").await;
    })
    .await;
    assert_eq!(line, "GET /_internal/repos/public/alice/pro%20j%3Fx HTTP/1.1");
}

#[tokio::test]
async fn merge_request_lookup_encodes_the_repo_and_keeps_the_number() {
    let line = request_line_of(async |core| {
        let _ = core.merge_request_detail("alice/proj&number=9", 3).await;
    })
    .await;
    assert_eq!(
        line,
        "GET /_internal/merge-requests?repo=alice%2Fproj%26number%3D9&number=3 HTTP/1.1"
    );
}

#[tokio::test]
async fn repo_name_with_a_dot_segment_is_refused_before_any_request() {
    let core = CoreClient::new("http://127.0.0.1:1/_internal".to_string(), "secret".to_string()).unwrap();
    assert!(core.public_repo("../../authorize").await.is_err());
}

#[tokio::test]
async fn a_core_that_never_answers_fails_within_the_request_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/_internal", listener.local_addr().unwrap());
    // Accepts and holds the socket open without ever replying.
    let _hung = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
        drop(socket);
    });
    let core = CoreClient::new(base, "secret".to_string()).unwrap();
    let started = std::time::Instant::now();
    let result = core.uid_for_name("alice").await;
    assert!(result.is_err());
    let limit = gitquarry_wire::ipc::LOOPBACK_REQUEST_TIMEOUT + std::time::Duration::from_secs(5);
    assert!(started.elapsed() < limit, "took {:?}", started.elapsed());
}
