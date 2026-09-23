//! A WebDAV server that refuses exactly one kind of request, on command: the
//! real fixture, behind an HTTP proxy this process owns.
//!
//! A server says 507 (Insufficient Storage) when a disk or an account quota
//! fills up, and 403 when a share's permissions change under a running copy.
//! Neither can be arranged on the shared Apache fixture without changing it for
//! every other suite leasing it (❌ never touch the container: other worktrees
//! use it at the same time), and an in-memory double would miss what matters
//! here: what `reqwest` and the backend make of a real refusal on a real
//! connection, mid-body included. So everything goes through to Apache
//! untouched except the one request a [`Refusal`] names, which the proxy
//! answers itself.
//!
//! One request per connection: the proxy asks Apache for `Connection: close`
//! and passes the answer back, so the client never reuses a connection and each
//! request arrives on a fresh one the proxy can judge from its head alone.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cmdr_fs::volume::host::VolumeHost;
use cmdr_fs::volume::host::credentials::InMemoryCredentials;
use cmdr_fs::volume::host::events::{RecordingVolumeEvents, VolumeEventSink};
use cmdr_webdav::volume::testing::{FIXTURE_USER, fixture_target};
use cmdr_webdav::{WebdavConnectionParams, WebdavVolume, connect_webdav_volume};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

/// How the proxy answers the request it refuses.
#[derive(Clone, Copy, Debug)]
pub(super) enum Answer {
    /// Reads the whole body first, then answers: a server that took the upload
    /// in and found no room to keep it.
    AfterTheBody,
    /// Answers the moment the request head is in and closes, leaving the body
    /// unread: a server that checks a quota against `Content-Length` up front,
    /// which is what a full Nextcloud account does.
    BeforeTheBody,
}

/// The one kind of request the proxy refuses.
#[derive(Clone, Copy, Debug)]
pub(super) struct Refusal {
    /// The HTTP method, as it appears on the request line (`PUT`, `MOVE`, ...).
    pub(super) method: &'static str,
    /// Which request paths it applies to.
    pub(super) path: PathRule,
    /// The status code the proxy answers with.
    pub(super) status: u16,
    pub(super) answer: Answer,
}

/// Which request paths a [`Refusal`] applies to, spelled as they go out on the
/// wire (percent-encoded; plain ASCII names need no encoding).
#[derive(Clone, Copy, Debug)]
pub(super) enum PathRule {
    /// The path carries this anywhere. A staging sibling carries its file's
    /// name, so this catches the upload of a file whatever temp it rides on.
    Contains(&'static str),
    /// The path ends with this: the file's own name, and none of its temps.
    EndsWith(&'static str),
}

impl Refusal {
    fn matches(&self, method: &str, path: &str) -> bool {
        method == self.method
            && match self.path {
                PathRule::Contains(piece) => path.contains(piece),
                PathRule::EndsWith(tail) => path.ends_with(tail),
            }
    }
}

/// The proxy, running until it's dropped.
pub(super) struct RefusingProxy {
    port: u16,
    refused: Arc<AtomicUsize>,
    stop: CancellationToken,
}

impl RefusingProxy {
    /// Starts a proxy in front of `upstream` that refuses what `refusal` names.
    pub(super) async fn start(upstream: SocketAddr, refusal: Refusal) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("a loopback port");
        let port = listener.local_addr().expect("a bound listener has an address").port();
        let refused = Arc::new(AtomicUsize::new(0));
        let stop = CancellationToken::new();
        let (accept_refused, accept_stop) = (Arc::clone(&refused), stop.clone());
        tokio::spawn(async move {
            loop {
                let client = tokio::select! {
                    () = accept_stop.cancelled() => return,
                    accepted = listener.accept() => match accepted {
                        Ok((client, _)) => client,
                        Err(_) => continue,
                    },
                };
                let refused = Arc::clone(&accept_refused);
                tokio::spawn(async move {
                    // A connection that goes wrong is the CLIENT's to report;
                    // the proxy has nothing to add.
                    let _ = serve_one(client, upstream, refusal, &refused).await;
                });
            }
        });
        Self { port, refused, stop }
    }

    /// How many requests the proxy has refused so far, so a cell can insist the
    /// refusal it arranged actually happened.
    pub(super) fn refused(&self) -> usize {
        self.refused.load(Ordering::SeqCst)
    }
}

impl Drop for RefusingProxy {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

/// Where a request head ends.
fn head_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

/// Serves one request on `client`: refuses it, or passes it to `upstream` with
/// `Connection: close` and streams both directions until either side closes.
async fn serve_one(
    mut client: TcpStream,
    upstream: SocketAddr,
    refusal: Refusal,
    refused: &AtomicUsize,
) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let end = loop {
        if let Some(end) = head_end(&buffer) {
            break end;
        }
        let read = client.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or_default();
    let path = parts.next().unwrap_or_default();
    let content_length: usize = lines
        .clone()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0);

    if refusal.matches(method, path) {
        refused.fetch_add(1, Ordering::SeqCst);
        if matches!(refusal.answer, Answer::AfterTheBody) {
            let mut remaining = content_length.saturating_sub(buffer.len() - end);
            while remaining > 0 {
                let read = client.read(&mut chunk).await?;
                if read == 0 {
                    break;
                }
                remaining = remaining.saturating_sub(read);
            }
        }
        let response = format!(
            "HTTP/1.1 {} Refused\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            refusal.status
        );
        client.write_all(response.as_bytes()).await?;
        client.shutdown().await?;
        return Ok(());
    }

    // Everything else goes through, with every `Connection` header swapped for
    // `close` so Apache ends the exchange and the client opens a fresh
    // connection for its next request.
    let mut forwarded = String::with_capacity(head.len() + 20);
    forwarded.push_str(request_line);
    forwarded.push_str("\r\n");
    for line in lines.filter(|l| !l.is_empty()) {
        if line
            .split_once(':')
            .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case("connection"))
        {
            continue;
        }
        forwarded.push_str(line);
        forwarded.push_str("\r\n");
    }
    forwarded.push_str("Connection: close\r\n\r\n");
    let mut server = TcpStream::connect(upstream).await?;
    server.write_all(forwarded.as_bytes()).await?;
    server.write_all(&buffer[end..]).await?;
    tokio::io::copy_bidirectional(&mut client, &mut server).await?;
    Ok(())
}

/// A volume on the stock Apache fixture, connected THROUGH a proxy that refuses
/// what `refusal` names. The proxy lives as long as the returned handle.
pub(super) async fn through_a_refusing_proxy(refusal: Refusal) -> (RefusingProxy, WebdavVolume) {
    let fixture = fixture_target("APACHE", 13480, FIXTURE_USER);
    let upstream = SocketAddr::from((
        [127, 0, 0, 1],
        fixture.base_url.port().expect("a fixture URL names its port"),
    ));
    let proxy = RefusingProxy::start(upstream, refusal).await;
    let mut base_url = fixture.base_url.clone();
    base_url.set_port(Some(proxy.port)).expect("an http URL takes a port");
    let params = WebdavConnectionParams::new(base_url, &fixture.username, &fixture.root);
    let host = VolumeHost::builder()
        .events(Arc::new(RecordingVolumeEvents::new()) as Arc<dyn VolumeEventSink>)
        .credentials(Arc::new(InMemoryCredentials::new().with_entry(
            &params.credential_service(),
            Some(&fixture.username),
            &fixture.username,
            &fixture.password,
        )))
        .build();
    let volume = connect_webdav_volume(
        "fixture",
        &format!("webdav-test-refusing-{}", proxy.port),
        params,
        host,
        CancellationToken::new(),
    )
    .await
    .unwrap_or_else(|e| {
        panic!(
            "the proxied fixture refused a connection ({e:?}); is the stack up? apps/desktop/test/webdav-servers/start.sh"
        )
    });
    (proxy, volume)
}
