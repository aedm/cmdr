//! What the client hands back, against a one-shot local server.

use std::time::Duration;

use reqwest::Method;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

use super::WebdavClient;

/// `cmdr stores these bytes verbatim\n` gzipped (`gzip -9n`): a file a server
/// hands out with `Content-Encoding: gzip`, as one serving `.gz` files or
/// compressing on the fly can.
const STORED_GZIP: &[u8] = &[
    0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x03, 0x4b, 0xce, 0x4d, 0x29, 0x52, 0x28, 0x2e, 0xc9, 0x2f,
    0x4a, 0x2d, 0x56, 0x28, 0xc9, 0x48, 0x2d, 0x4e, 0x55, 0x48, 0xaa, 0x2c, 0x01, 0xb2, 0xcb, 0x52, 0x8b, 0x92, 0x12,
    0x4b, 0x32, 0x73, 0xb9, 0x00, 0xce, 0xed, 0x88, 0x3e, 0x21, 0x00, 0x00, 0x00,
];

/// A server answering `count` requests with the gzip body and its headers
/// (no body for a HEAD), and a client pointed at it.
async fn serving_stored_gzip(count: usize) -> (WebdavClient, Url) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for _ in 0..count {
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
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-encoding: gzip\r\ncontent-length: {}\r\n\
                 connection: close\r\n\r\n",
                STORED_GZIP.len()
            );
            let _ = socket.write_all(head.as_bytes()).await;
            if !buffer.starts_with(b"HEAD") {
                let _ = socket.write_all(STORED_GZIP).await;
            }
            let _ = socket.shutdown().await;
        }
    });
    let base = Url::parse(&format!("http://{addr}/dav/")).unwrap();
    (
        WebdavClient::new(base.clone(), "user", "secret").unwrap(),
        base.join("page.html").unwrap(),
    )
}

/// ❗ A file manager copies bytes, it doesn't decode them: a file served with
/// `Content-Encoding: gzip` reads back as the bytes the server holds, at the
/// length its `Content-Length` says. The test build has every reqwest decoder
/// on (`Cargo.toml`'s dev-dependencies), as the app's graph may: the client
/// must turn each one off itself.
#[tokio::test]
async fn an_encoded_file_reads_back_as_its_stored_bytes() {
    let (client, url) = serving_stored_gzip(2).await;

    let get = client.send(client.request(Method::GET, url.clone())).await.unwrap();
    let body = tokio::time::timeout(Duration::from_secs(5), get.bytes())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(body.as_ref(), STORED_GZIP, "a GET");

    let head = client.send(client.request(Method::HEAD, url)).await.unwrap();
    assert_eq!(
        head.headers().get("content-length").and_then(|v| v.to_str().ok()),
        Some(STORED_GZIP.len().to_string().as_str()),
        "a HEAD keeps the length"
    );
}

/// ❗ This crate builds reqwest with `http2` itself, so its own tests speak
/// what the app negotiates (the app gets the feature through `genai`).
/// `http2_prior_knowledge` exists only with the feature, so dropping it from
/// `Cargo.toml` fails this file to compile.
#[test]
fn the_client_is_built_with_http2() {
    assert!(reqwest::Client::builder().http2_prior_knowledge().build().is_ok());
}
