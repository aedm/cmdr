//! A server that REALLY goes away under a live volume, and comes back.
//!
//! `reconnect_test.rs` drives the switch by dropping the client in-process
//! (`simulate_session_loss`), which proves the policy and never the wire. These
//! cells cut the actual TCP connection instead, which is what a user hits: a NAS
//! that sleeps or reboots, Wi-Fi that drops.
//!
//! ❗ The cut happens in a `cmdr_fs::testing::tcp_proxy::TcpProxy` this test owns,
//! between the client and `webdav-fixture-apache`. ❌ Never pause or stop the
//! container: the stack is shared by lease with other test binaries, worktrees,
//! and sessions, and every one of them would see the outage too.
//!
//! Two ways down, because they end differently:
//!
//! - **Refused**: the pooled connection closes and a new one is refused.
//! - **Silent** (a black hole): nothing closes, nothing answers, so only the
//!   request's own budget ends the wait. It runs under a paused tokio clock, so
//!   the production-length budget elapses in virtual time.
//!
//! Every cell here needs the WebDAV fixture stack:
//! `apps/desktop/test/webdav-servers/start.sh`. Against a server of your own
//! (`CMDR_WEBDAV_TEST_URL`) they skip: there's no fixture to proxy.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use cmdr_fs::testing::tcp_proxy::TcpProxy;
use cmdr_fs::testing::wait_until_async;
use cmdr_fs::volume::host::VolumeHost;
use cmdr_fs::volume::host::credentials::InMemoryCredentials;
use cmdr_fs::volume::host::events::{RecordingVolumeEvents, VolumeConnection, VolumeEventSink};
use cmdr_fs::volume::{ConnectionState, Volume, VolumeError};
use tokio_util::sync::CancellationToken;

use super::testing::*;
use super::{WebdavVolume, connect_webdav_volume};
use crate::params::WebdavConnectionParams;

const FIXTURE: &str = "webdav-servers/start.sh (webdav-fixture)";

/// How long a PROPFIND may run before it's given up on
/// (`transport::PROPFIND_BUDGET`). The silent cell holds the listing to it.
const PROPFIND_BUDGET: Duration = Duration::from_secs(60);

/// How long an operation on a REFUSED connection may take to answer. Generous:
/// the real answer is milliseconds, and this is a hang backstop that keeps the
/// failure message ours rather than nextest's.
const ANSWERS_WITHIN: Duration = Duration::from_secs(3);

/// How long the backend's own loop gets to bring a volume back: its first
/// backoff step (2 s) plus a probe, under the 8 s nextest cap.
const COMES_BACK_WITHIN: Duration = Duration::from_secs(6);

/// A volume on `webdav-fixture-apache`, connected THROUGH a proxy this test owns.
struct Proxied {
    proxy: TcpProxy,
    events: Arc<RecordingVolumeEvents>,
    volume: WebdavVolume,
}

/// Connects through a fresh proxy, secret remembered.
///
/// ❗ The host runs background work on THIS runtime (`Handle::current`), so a
/// paused clock pauses the backoff loop along with the proxy and the request.
async fn through_a_proxy() -> Proxied {
    let fixture = fixture_target("APACHE", 13480, FIXTURE_USER);
    let target = SocketAddr::from((
        [127, 0, 0, 1],
        fixture.base_url.port().expect("a fixture URL names its port"),
    ));
    let proxy = TcpProxy::start(target).await;
    let mut base_url = fixture.base_url.clone();
    base_url.set_port(Some(proxy.port())).expect("an http URL takes a port");
    let params = WebdavConnectionParams::new(base_url, &fixture.username, &fixture.root);
    let events = Arc::new(RecordingVolumeEvents::new());
    let host = VolumeHost::builder()
        .runtime(tokio::runtime::Handle::current())
        .events(Arc::clone(&events) as Arc<dyn VolumeEventSink>)
        .credentials(Arc::new(InMemoryCredentials::new().with_entry(
            &params.credential_service(),
            Some(&fixture.username),
            &fixture.username,
            &fixture.password,
        )))
        .build();
    let volume = connect_webdav_volume(
        "fixture",
        &format!("webdav-test-proxied-{}", proxy.port()),
        params,
        host,
        CancellationToken::new(),
    )
    .await
    .unwrap_or_else(|e| panic!("the proxied fixture refused a connection ({e:?}); is the stack up? {FIXTURE}"));
    Proxied { proxy, events, volume }
}

/// The root's seeded names, sorted, or the error the listing answered.
///
/// ❗ Scratch directories left out: other cells create and delete theirs in
/// the same export while this one runs, so two listings a few seconds apart
/// legitimately differ there.
async fn names(volume: &WebdavVolume) -> Result<Vec<String>, VolumeError> {
    let mut names: Vec<String> = volume
        .list_directory(volume.root(), None)
        .await?
        .into_iter()
        .map(|entry| entry.name)
        .filter(|name| !name.starts_with("cmdr-test-"))
        .collect();
    names.sort();
    Ok(names)
}

/// The states a volume reported, in order.
fn reported(events: &RecordingVolumeEvents) -> Vec<VolumeConnection> {
    events.transitions().into_iter().map(|(_, state)| state).collect()
}

/// ❗ **A server that goes away is reported down at once, with a typed error,
/// and comes back on its own once it's reachable again.**
///
/// There is no watcher, so the first request after the drop is the detector: it
/// has to answer `DeviceDisconnected` promptly, the one error that flips the
/// state and starts the backoff. A retry while the server is still gone answers
/// the same way and reports nothing new. Then the server returns and the
/// backend's own loop, with nobody asking, brings the SAME volume back.
#[tokio::test]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn a_server_that_goes_away_is_reported_down_and_comes_back_on_its_own() {
    if not_for_your_own_server("a Docker fixture to put a proxy in front of") {
        return;
    }
    let Proxied { proxy, events, volume } = through_a_proxy().await;
    let before = names(&volume).await.expect(FIXTURE);

    proxy.refuse().await;

    let failed = tokio::time::timeout(ANSWERS_WITHIN, names(&volume))
        .await
        .expect("❗ a request to a refusing server has to answer, not hang");
    assert!(
        matches!(failed, Err(VolumeError::DeviceDisconnected(_))),
        "only DeviceDisconnected flips the state and starts the backoff, got {failed:?}"
    );
    assert_eq!(volume.connection_state(), Some(ConnectionState::Disconnected));
    assert_eq!(reported(&events), vec![VolumeConnection::Disconnected]);

    // The frontend's own backoff tick, while the server is still gone.
    let retry = tokio::time::timeout(ANSWERS_WITHIN, volume.attempt_reconnect())
        .await
        .expect("a probe of a refusing server answers, not hangs");
    assert!(
        matches!(retry, Err(VolumeError::DeviceDisconnected(_))),
        "a refused probe is transient, never a sign-in: {retry:?}"
    );
    assert_eq!(
        reported(&events),
        vec![VolumeConnection::Disconnected],
        "a failed retry is not news"
    );

    proxy.restore().await;

    wait_until_async(COMES_BACK_WITHIN, "the backoff loop to bring the volume back", || {
        reported(&events).contains(&VolumeConnection::Connected)
    })
    .await;
    assert_eq!(volume.connection_state(), Some(ConnectionState::Direct));
    assert_eq!(
        names(&volume).await.expect(FIXTURE),
        before,
        "the same volume lists again"
    );
    assert_eq!(
        reported(&events),
        vec![VolumeConnection::Disconnected, VolumeConnection::Connected],
        "one drop, one recovery"
    );
}

/// ❗ **With "Reconnect automatically" off, a server that comes back is left
/// alone until someone asks.**
///
/// The proxy counts every connection it accepts, so "nothing probed" is a
/// number, not an absence of events. The window runs past the backoff's first
/// step (2 s), which is when a loop that shouldn't exist would have probed.
#[tokio::test]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn with_the_switch_off_a_returning_server_stays_down_until_asked() {
    if not_for_your_own_server("a Docker fixture to put a proxy in front of") {
        return;
    }
    let Proxied { proxy, events, volume } = through_a_proxy().await;
    volume.set_auto_reconnect(false);

    proxy.refuse().await;
    let failed = tokio::time::timeout(ANSWERS_WITHIN, names(&volume))
        .await
        .expect("❗ a request to a refusing server has to answer, not hang");
    assert!(matches!(failed, Err(VolumeError::DeviceDisconnected(_))), "{failed:?}");
    proxy.restore().await;
    let connections_before = proxy.connections_accepted();

    // allowed-test-sleep: a negative assertion over a window. Nothing can signal "no probe happened"; the window runs
    // past the backoff's first 2 s step, which is when a loop that shouldn't exist would have probed.
    tokio::time::sleep(Duration::from_millis(2_500)).await;

    assert_eq!(
        proxy.connections_accepted(),
        connections_before,
        "❌ nothing may probe unattended with the switch off, however reachable the server is"
    );
    assert_eq!(volume.connection_state(), Some(ConnectionState::Disconnected));
    assert!(
        matches!(volume.attempt_reconnect().await, Err(VolumeError::NotSupported)),
        "and an unattended ask is refused by the switch, not by the server"
    );

    // The user signs in by hand.
    volume
        .reconnect_with_credentials(FIXTURE_USER.to_string(), FIXTURE_PASSWORD.to_string())
        .await
        .expect(FIXTURE);
    assert!(names(&volume).await.is_ok(), "an attended reconnect brings it back");
    assert_eq!(
        reported(&events),
        vec![VolumeConnection::Disconnected, VolumeConnection::Connected]
    );
}

/// ❗ **A server that goes SILENT is given up on at the listing's budget, not
/// waited on forever**, and the volume serves again once the path clears. It
/// never reports itself down on the way, which is a known gap (see the
/// assertion).
///
/// Nothing closes in a black hole, so no error ever arrives on its own: only
/// the request's budget can end the wait. The clock is paused for exactly the
/// silent stretch, so the 60 s budget elapses in virtual time and a missing one
/// shows up as the five-minute backstop firing rather than as a hung test.
///
/// ❗ Resumed before the proxy is restored: nothing after it may run on a clock
/// that jumps whenever the runtime waits on the network.
#[tokio::test]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn a_server_that_goes_silent_is_given_up_on_and_serves_again() {
    if not_for_your_own_server("a Docker fixture to put a proxy in front of") {
        return;
    }
    let Proxied { proxy, events, volume } = through_a_proxy().await;
    let before = names(&volume).await.expect(FIXTURE);

    proxy.black_hole();
    tokio::time::pause();
    let started = tokio::time::Instant::now();
    let failed = tokio::time::timeout(Duration::from_secs(300), names(&volume)).await;
    let waited = started.elapsed();
    tokio::time::resume();

    let failed = failed.expect("❗ a listing on a silent server waited five minutes: nothing bounds it");
    assert!(
        matches!(failed, Err(VolumeError::ConnectionTimeout(_))),
        "a server that never answered is a typed timeout: {failed:?}"
    );
    assert!(
        waited <= PROPFIND_BUDGET + Duration::from_secs(1),
        "given up on after {waited:?}, past the {PROPFIND_BUDGET:?} a listing gets"
    );
    assert!(
        events.transitions().is_empty(),
        "⚠️ the known gap, pinned so closing it is a deliberate change: a timeout doesn't flip the state \
         (`crates/cmdr-webdav/DETAILS.md` § \"The reconnect model\")"
    );

    proxy.restore().await;

    let after = tokio::time::timeout(ANSWERS_WITHIN, names(&volume))
        .await
        .expect("the path is clear again");
    assert_eq!(after.expect(FIXTURE), before, "the same volume lists again");
}
