use aether_cli::acp::testing::{AcpTestHarness, AcpWebSocketTestServer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::http::StatusCode;

#[tokio::test(flavor = "current_thread")]
async fn health_probe_is_answered_with_ok() {
    AcpTestHarness::run(|mut harness| async move {
        let server = harness.serve_websocket().await;

        let response = probe(&server, "GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n").await;

        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert!(response.ends_with("ok"), "{response}");
        server.shutdown().await;
        harness.shutdown().await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn health_probe_does_not_take_the_client_slot() {
    AcpTestHarness::run(|mut harness| async move {
        let server = harness.serve_websocket().await;
        let (socket, response) = connect_async(server.url()).await.expect("websocket client connects");
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);

        let probe = probe(&server, "GET /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n").await;

        assert!(probe.starts_with("HTTP/1.1 200 OK"), "{probe}");
        drop(socket);
        server.shutdown().await;
        harness.shutdown().await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn other_paths_are_left_to_the_websocket_handshake() {
    AcpTestHarness::run(|mut harness| async move {
        let server = harness.serve_websocket().await;

        let response = probe(&server, "GET /not-healthz HTTP/1.1\r\nHost: localhost\r\n\r\n").await;

        assert!(!response.starts_with("HTTP/1.1 200 OK"), "{response}");
        server.shutdown().await;
        harness.shutdown().await;
    })
    .await;
}

async fn probe(server: &AcpWebSocketTestServer, request: &str) -> String {
    let mut stream = TcpStream::connect(server.address).await.expect("health probe connects");
    stream.write_all(request.as_bytes()).await.expect("health probe writes");
    let mut response = String::new();
    stream.read_to_string(&mut response).await.expect("health probe reads");
    response
}
