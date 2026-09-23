//! A TCP proxy a test owns and can cut, for testing what a network backend does
//! when its server really goes away.
//!
//! The Docker fixture stacks are SHARED: other test binaries, other worktrees,
//! and other sessions hold leases on the same containers at the same time, so
//! pausing or stopping one to simulate an outage would break every concurrent
//! run. A proxy in the test's own process sits between the client and the
//! fixture instead. Cutting it affects exactly one test's connections, needs no
//! Docker control, and is instant and deterministic.
//!
//! ```ignore
//! let proxy = TcpProxy::start(fixture_addr).await;
//! let params = params_for("127.0.0.1", proxy.port());
//! // ... connect through it, then:
//! proxy.refuse().await; // the server is down: live connections close, new ones are refused
//! proxy.restore().await; // it's back
//! ```
//!
//! Two ways down, because users hit both and a backend handles them differently:
//!
//! - [`TcpProxy::refuse`]: the server went away cleanly (a NAS rebooting, a
//!   daemon restarting). Live connections close under the client, and a new
//!   dial is refused (`ECONNREFUSED`), which is the answer a host gives for a
//!   closed port.
//! - [`TcpProxy::black_hole`]: the path went silent (a NAS asleep, Wi-Fi gone,
//!   a VPN dropped). Nothing closes and nothing answers: bytes sit where they
//!   are, and a new dial connects and then hears nothing. This is the case that
//!   finds a missing deadline, because nothing but a timer ever ends a wait.
//!   Pair it with `tokio::time::pause()` so a production-length deadline
//!   elapses in virtual time rather than wall-clock.
//!
//! ❗ The proxy runs on the ambient tokio runtime (the test's own), so a paused
//! clock pauses it too. It does no timing of its own, which is what keeps it
//! correct under one.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::ignore_poison::IgnorePoison;

/// Whether bytes move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Link {
    Up,
    BlackHole,
}

/// A proxy from a loopback port of its own to one real server.
///
/// Dropping it closes everything it holds.
pub struct TcpProxy {
    port: u16,
    target: SocketAddr,
    link: watch::Sender<Link>,
    accepted: Arc<AtomicUsize>,
    connections: Arc<Mutex<Vec<JoinHandle<()>>>>,
    /// The accept loop. `None` while refusing, which is what makes the port
    /// refuse: nothing is listening on it.
    listener: Mutex<Option<JoinHandle<()>>>,
}

impl TcpProxy {
    /// Starts forwarding from a fresh `127.0.0.1` port to `target`.
    ///
    /// ❗ Call it from inside the runtime the test runs on: the proxy's tasks
    /// spawn onto the ambient one.
    pub async fn start(target: SocketAddr) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("binding the test proxy to a loopback port");
        let port = listener.local_addr().expect("the proxy's own address").port();
        let (link, _) = watch::channel(Link::Up);
        let proxy = Self {
            port,
            target,
            link,
            accepted: Arc::new(AtomicUsize::new(0)),
            connections: Arc::new(Mutex::new(Vec::new())),
            listener: Mutex::new(None),
        };
        proxy.serve(listener);
        proxy
    }

    /// The loopback port to point the client at.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// How many connections the proxy has accepted so far, in any state.
    ///
    /// The evidence for "nothing dialed": a reconnect that the test expects NOT
    /// to happen would show up here as one more.
    pub fn connections_accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    /// The server goes away: every live connection closes, and every new dial
    /// is refused until [`Self::restore`].
    ///
    /// Returns once the sockets are actually closed, so the test's next step
    /// already sees the server gone.
    pub async fn refuse(&self) {
        let listener = self.listener.lock_ignore_poison().take();
        if let Some(listener) = listener {
            listener.abort();
            let _ = listener.await;
        }
        let connections: Vec<_> = self.connections.lock_ignore_poison().drain(..).collect();
        for connection in connections {
            connection.abort();
            let _ = connection.await;
        }
    }

    /// The path goes silent: nothing closes and nothing answers.
    ///
    /// Bytes already in flight stay held, and a new dial connects and then
    /// hears nothing, until [`Self::restore`].
    pub fn black_hole(&self) {
        self.link.send_replace(Link::BlackHole);
    }

    /// The server is back: listening again on the same port, and forwarding
    /// on every connection that survived.
    ///
    /// ❗ Rebinds the SAME port, because the client has it in its connection
    /// parameters. A port another process grabbed in the gap fails the test
    /// loudly rather than pointing the client somewhere else.
    pub async fn restore(&self) {
        let needs_listener = self.listener.lock_ignore_poison().is_none();
        if needs_listener {
            let listener = TcpListener::bind(("127.0.0.1", self.port))
                .await
                .unwrap_or_else(|e| panic!("rebinding the test proxy's port {}: {e}", self.port));
            self.serve(listener);
        }
        self.link.send_replace(Link::Up);
    }

    fn serve(&self, listener: TcpListener) {
        let target = self.target;
        let link = self.link.clone();
        let accepted = Arc::clone(&self.accepted);
        let connections = Arc::clone(&self.connections);
        let task = tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                accepted.fetch_add(1, Ordering::SeqCst);
                let link = link.subscribe();
                let connection = tokio::spawn(forward(client, target, link));
                let mut held = connections.lock_ignore_poison();
                held.retain(|task| !task.is_finished());
                held.push(connection);
            }
        });
        *self.listener.lock_ignore_poison() = Some(task);
    }
}

impl Drop for TcpProxy {
    fn drop(&mut self) {
        if let Some(listener) = self.listener.lock_ignore_poison().take() {
            listener.abort();
        }
        for connection in self.connections.lock_ignore_poison().drain(..) {
            connection.abort();
        }
    }
}

/// One client connection, carried to the server both ways until either side
/// closes or the proxy drops it.
async fn forward(client: TcpStream, target: SocketAddr, link: watch::Receiver<Link>) {
    let Ok(server) = TcpStream::connect(target).await else {
        // The real server refused: pass that on by closing the client.
        return;
    };
    let (client_read, client_write) = client.into_split();
    let (server_read, server_write) = server.into_split();
    tokio::join!(
        pump(client_read, server_write, link.clone()),
        pump(server_read, client_write, link),
    );
}

/// Copies one direction, holding every byte while the link is black-holed.
///
/// ❗ The wait sits on BOTH sides of the read: a read already parked when the
/// hole opens still completes, and the second wait is what keeps those bytes
/// from getting through.
async fn pump(mut from: OwnedReadHalf, mut to: OwnedWriteHalf, mut link: watch::Receiver<Link>) {
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        if link.wait_for(|state| *state == Link::Up).await.is_err() {
            return;
        }
        let read = match from.read(&mut buffer).await {
            Ok(0) | Err(_) => {
                let _ = to.shutdown().await;
                return;
            }
            Ok(read) => read,
        };
        if link.wait_for(|state| *state == Link::Up).await.is_err() {
            return;
        }
        if to.write_all(&buffer[..read]).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
#[path = "tcp_proxy_test.rs"]
mod tcp_proxy_test;
