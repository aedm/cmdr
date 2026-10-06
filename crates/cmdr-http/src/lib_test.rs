//! The built client end to end: a fake proxy and a fake origin on loopback, and which one each
//! request reaches.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;

/// A one-route system layer, so the test doesn't depend on this Mac's settings.
struct FixedSystem(Route);

impl SystemProxies for FixedSystem {
    fn route(&self, _url: &Url) -> Route {
        self.0.clone()
    }
}

/// An HTTP/1.1 server on `127.0.0.1` that answers every request with `name` as the body and
/// records each request line.
async fn server(name: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a loopback port");
    let address = listener.local_addr().expect("a bound address");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && socket.read(&mut byte).await.unwrap_or(0) == 1 {
                    head.push(byte[0]);
                }
                let line = String::from_utf8_lossy(&head)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_string();
                log.lock().expect("an unpoisoned test log").push(line);
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{name}",
                    name.len()
                );
                let _ = socket.write_all(reply.as_bytes()).await;
            });
        }
    });
    (format!("http://{address}"), seen)
}

fn env(vars: &[(&str, &str)]) -> Arc<EnvProxies> {
    Arc::new(EnvProxies::from_vars(|name| {
        vars.iter().find(|(n, _)| *n == name).map(|(_, v)| v.to_string())
    }))
}

async fn body(client: &reqwest::Client, url: &str) -> Result<String, reqwest::Error> {
    client.get(url).send().await?.text().await
}

#[tokio::test]
async fn loopback_goes_direct_even_with_a_proxy_set() {
    let (proxy, proxy_seen) = server("proxy").await;
    let (origin, _) = server("origin").await;
    let client = builder_with(
        env(&[("HTTP_PROXY", &proxy)]),
        Arc::new(FixedSystem(Route::Proxy(proxy.clone()))),
    )
    .build()
    .expect("a client");

    assert_eq!(
        body(&client, &format!("{origin}/health"))
            .await
            .expect("a direct answer"),
        "origin"
    );
    assert!(proxy_seen.lock().expect("an unpoisoned test log").is_empty());
}

#[tokio::test]
async fn other_hosts_go_through_the_environment_proxy() {
    let (proxy, proxy_seen) = server("proxy").await;
    let client = builder_with(env(&[("HTTP_PROXY", &proxy)]), Arc::new(FixedSystem(Route::Direct)))
        .build()
        .expect("a client");

    assert_eq!(
        body(&client, "http://cmdr.invalid/x")
            .await
            .expect("the proxy's answer"),
        "proxy"
    );
    assert_eq!(
        *proxy_seen.lock().expect("an unpoisoned test log"),
        ["GET http://cmdr.invalid/x HTTP/1.1"]
    );
}

#[tokio::test]
async fn the_system_proxy_carries_what_the_environment_leaves() {
    let (proxy, proxy_seen) = server("proxy").await;
    let client = builder_with(env(&[]), Arc::new(FixedSystem(Route::Proxy(proxy.clone()))))
        .build()
        .expect("a client");

    assert_eq!(
        body(&client, "http://cmdr.invalid/y")
            .await
            .expect("the proxy's answer"),
        "proxy"
    );
    assert_eq!(
        *proxy_seen.lock().expect("an unpoisoned test log"),
        ["GET http://cmdr.invalid/y HTTP/1.1"]
    );
}

#[tokio::test]
async fn no_proxy_sends_a_host_direct_past_both_proxies() {
    let (proxy, proxy_seen) = server("proxy").await;
    let client = builder_with(
        env(&[("HTTP_PROXY", &proxy), ("NO_PROXY", "cmdr.invalid")]),
        Arc::new(FixedSystem(Route::Proxy(proxy.clone()))),
    )
    .build()
    .expect("a client");

    // Direct to a `.invalid` host can't resolve, which is the point: it never reached the proxy.
    assert!(body(&client, "http://cmdr.invalid/z").await.is_err());
    assert!(proxy_seen.lock().expect("an unpoisoned test log").is_empty());
}
