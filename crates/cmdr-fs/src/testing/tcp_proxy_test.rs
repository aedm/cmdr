//! The proxy's own promises, against an echo server in the same process.
//!
//! Every network backend's drop cells lean on these: a proxy that quietly kept
//! forwarding while "refusing" would let a reconnect test pass without the
//! server ever having gone away.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::TcpProxy;

/// An echo server on a loopback port, for as long as the runtime lives.
async fn echo_server() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind echo");
    let addr = listener.local_addr().expect("echo addr");
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buffer = [0u8; 1024];
                while let Ok(read) = socket.read(&mut buffer).await {
                    if read == 0 || socket.write_all(&buffer[..read]).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

/// Sends `bytes` and reads the echo back.
async fn round_trip(stream: &mut TcpStream, bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    stream.write_all(bytes).await?;
    let mut back = vec![0u8; bytes.len()];
    stream.read_exact(&mut back).await?;
    Ok(back)
}

async fn dial(proxy: &TcpProxy) -> std::io::Result<TcpStream> {
    TcpStream::connect(("127.0.0.1", proxy.port())).await
}

#[tokio::test]
async fn it_forwards_both_ways() {
    let proxy = TcpProxy::start(echo_server().await).await;
    let mut stream = dial(&proxy).await.expect("dial the proxy");

    assert_eq!(round_trip(&mut stream, b"hello").await.expect("echo"), b"hello");
    assert_eq!(proxy.connections_accepted(), 1);
}

/// ❗ Refusing is the server GONE: the live connection closes, and a new dial
/// is refused outright rather than accepted and dropped.
#[tokio::test]
async fn refusing_closes_live_connections_and_refuses_new_ones() {
    let proxy = TcpProxy::start(echo_server().await).await;
    let mut stream = dial(&proxy).await.expect("dial the proxy");
    round_trip(&mut stream, b"up").await.expect("echo while up");

    proxy.refuse().await;

    let mut rest = Vec::new();
    let read = stream.read_to_end(&mut rest).await;
    assert!(
        matches!(read, Ok(0)) || read.is_err(),
        "the live connection must end, got {read:?}"
    );
    let refused = dial(&proxy).await;
    assert_eq!(
        refused.map(|_| ()).map_err(|e| e.kind()),
        Err(std::io::ErrorKind::ConnectionRefused),
        "nothing listens while refusing"
    );
}

/// Back up on the SAME port, which is what a client with that port in its
/// parameters needs.
#[tokio::test]
async fn restoring_listens_again_on_the_same_port() {
    let proxy = TcpProxy::start(echo_server().await).await;
    let port = proxy.port();
    proxy.refuse().await;

    proxy.restore().await;

    assert_eq!(proxy.port(), port);
    let mut stream = dial(&proxy).await.expect("dial the restored proxy");
    assert_eq!(round_trip(&mut stream, b"back").await.expect("echo"), b"back");
}

/// ❗ A black hole answers NOTHING and closes nothing, on a live connection and
/// a new one alike, and restoring lets the held bytes through.
///
/// The paused clock is how "nothing, ever" is asserted without waiting: a
/// read that could only end on a timer sees the timer fire in virtual time.
#[tokio::test]
async fn a_black_hole_holds_every_byte_until_restored() {
    let proxy = TcpProxy::start(echo_server().await).await;
    let mut live = dial(&proxy).await.expect("dial the proxy");
    round_trip(&mut live, b"up").await.expect("echo while up");

    proxy.black_hole();
    let mut fresh = dial(&proxy).await.expect("a black hole still accepts");

    tokio::time::pause();
    let silent = tokio::time::timeout(Duration::from_secs(3600), round_trip(&mut live, b"lost")).await;
    assert!(silent.is_err(), "the live connection heard something: {silent:?}");
    let silent = tokio::time::timeout(Duration::from_secs(3600), round_trip(&mut fresh, b"lost")).await;
    assert!(silent.is_err(), "the new connection heard something: {silent:?}");
    tokio::time::resume();

    proxy.restore().await;

    let mut held = [0u8; 4];
    live.read_exact(&mut held).await.expect("the held echo arrives");
    assert_eq!(&held, b"lost", "the bytes held in the hole get through once it closes");
}
