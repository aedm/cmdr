//! What the transport puts on the wire, read back by a one-shot local server.

use std::time::Duration;

use http::Method;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

use super::S3Client;
use crate::profile::{Preset, ProviderProfile};
use crate::request::S3Request;
use crate::sigv4::Credentials;

/// Serves one request with `204` and hands back its head, header names
/// lowercased.
async fn one_request_head(send: impl AsyncFnOnce(S3Client, String)) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        while !buffer.windows(4).any(|w| w == b"\r\n\r\n") {
            let read = socket.read(&mut chunk).await.unwrap();
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
        }
        let _ = socket
            .write_all(b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n")
            .await;
        String::from_utf8_lossy(&buffer).to_lowercase()
    });
    let profile = ProviderProfile::from_preset(&Preset::Other {
        endpoint: Url::parse(&format!("http://{addr}")).unwrap(),
        region: None,
        path_style: true,
    })
    .unwrap();
    let host = profile.endpoint_host.clone();
    send(
        S3Client::new(profile, Credentials::new("AKID", "secret")).unwrap(),
        host,
    )
    .await;
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap()
}

/// ❗ GCS answers `411 Length Required` to a bodyless POST without
/// `Content-Length` (`CreateMultipartUpload`; observed live on 2026-10-02).
#[tokio::test]
async fn a_bodyless_post_says_its_length_is_zero() {
    let head = one_request_head(async |client, host| {
        let request = S3Request::new(Method::POST, "http", &host, "/b/k".into()).query("uploads", "");
        client.exchange(request, Duration::from_secs(5)).await.unwrap();
    })
    .await;
    assert!(head.contains("\r\ncontent-length: 0\r\n"), "{head}");
}
