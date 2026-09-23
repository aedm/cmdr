//! The "reconnect automatically" switch on a mounted volume
//! (`WebdavVolume::set_auto_reconnect`): off means no unattended probe at all,
//! and back on acts at once.
//!
//! The cells without a server drive the gate on a volume with no client; the
//! `#[ignore]`d ones are Docker cells against the stock Apache fixture
//! (`apps/desktop/test/webdav-servers/start.sh`).

use std::sync::Arc;
use std::time::Duration;

use cmdr_fs::volume::host::VolumeHost;
use cmdr_fs::volume::host::credentials::InMemoryCredentials;
use cmdr_fs::volume::{ConnectionState, Volume, VolumeError};

use super::UnattendedReconnect;
use super::test_support::{make_test_volume, make_test_volume_with};
use super::testing::*;

const FIXTURE: &str = "webdav-servers/start.sh (webdav-fixture)";

// ── Without a server ─────────────────────────────────────────────────

/// ❗ **Off means no unattended probe happens AT ALL.**
///
/// The evidence is the typed refusal: this volume has nothing stored, so a
/// probe that got past the gate could only answer `PermissionDenied`.
/// `NotSupported` has exactly one source, and it is the switch.
#[tokio::test]
async fn the_switch_off_stops_an_unattended_probe_before_it_starts() {
    let volume = make_test_volume("/");
    volume.set_auto_reconnect(false);

    let refusal = volume.attempt_reconnect().await;

    assert!(
        matches!(refusal, Err(VolumeError::NotSupported)),
        "only the switch answers NotSupported; got {refusal:?}"
    );
}

/// Back on hands the probe its ordinary terms again, rather than leaving a
/// volume stuck on the answer the switch produced.
#[tokio::test]
async fn the_switch_back_on_lets_the_probe_through_again() {
    let volume = make_test_volume("/");
    volume.set_auto_reconnect(false);
    assert!(matches!(
        volume.attempt_reconnect().await,
        Err(VolumeError::NotSupported)
    ));

    volume.set_auto_reconnect(true);

    let refusal = volume.attempt_reconnect().await;
    assert!(
        matches!(refusal, Err(VolumeError::PermissionDenied(_))),
        "past the gate, an empty store is what answers; got {refusal:?}"
    );
}

/// ❗ **What the frontend reads follows the switch**, and the store only when
/// the switch is on.
#[tokio::test]
async fn the_readiness_answer_follows_the_switch_and_the_store() {
    let stored = InMemoryCredentials::new().with_entry("http://127.0.0.1:1", Some("ada"), "ada", "hunter2");
    let volume = make_test_volume_with("/", VolumeHost::builder().credentials(Arc::new(stored)).build());
    assert_eq!(volume.unattended_reconnect().await, UnattendedReconnect::Possible);

    volume.set_auto_reconnect(false);
    assert_eq!(volume.unattended_reconnect().await, UnattendedReconnect::SwitchOff);

    volume.set_auto_reconnect(true);
    assert_eq!(volume.unattended_reconnect().await, UnattendedReconnect::Possible);

    let empty = make_test_volume("/");
    assert_eq!(empty.unattended_reconnect().await, UnattendedReconnect::NoStoredSecret);
}

// ── Against the fixture ──────────────────────────────────────────────

/// ❗ **The switch off leaves a perfectly reachable server alone.**
///
/// Every ingredient of a successful reconnect is present: the server is up and
/// the secret is stored. The volume stays down anyway, and comes back once the
/// switch does.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn the_switch_off_leaves_a_reachable_server_alone() {
    if not_for_your_own_server("the seeded `hello.txt`") {
        return;
    }
    let volume = connect_fixture("APACHE", 13480).await;
    let hello = volume.root().join("hello.txt");
    volume.set_auto_reconnect(false);
    assert_eq!(volume.unattended_reconnect().await, UnattendedReconnect::SwitchOff);

    volume.simulate_session_loss().await;

    let refusal = volume.attempt_reconnect().await;
    assert!(
        matches!(refusal, Err(VolumeError::NotSupported)),
        "❌ no probe, however easy one would have been: {refusal:?}"
    );
    assert!(!volume.exists(&hello).await, "and the volume is genuinely still down");

    volume.set_auto_reconnect(true);
    volume.attempt_reconnect().await.expect(FIXTURE);
    assert!(volume.exists(&hello).await);
}

/// ❗ **Switching it back on brings a volume that is already down back**, on
/// its own.
///
/// The one moment the switch would look broken if it only took effect on the
/// NEXT drop: a user flips it on precisely because a volume is sitting there
/// disconnected. The wait covers the first backoff step (2 s) plus a probe, and
/// ❗ stays under nextest's 8 s cap, so a regression fails with its own message
/// rather than as an opaque timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the WebDAV fixture stack: apps/desktop/test/webdav-servers/start.sh (webdav-fixture)"]
async fn switching_the_switch_on_revives_a_volume_that_is_already_down() {
    if not_for_your_own_server("the seeded `hello.txt`") {
        return;
    }
    let volume = connect_fixture("APACHE", 13480).await;
    let hello = volume.root().join("hello.txt");
    volume.set_auto_reconnect(false);

    // The client drops and an operation NOTICES, which is what leaves the volume
    // `Disconnected`. With the switch off, no loop starts.
    volume.simulate_session_loss().await;
    assert!(!volume.exists(&hello).await);
    assert!(matches!(
        volume.attempt_reconnect().await,
        Err(VolumeError::NotSupported)
    ));
    assert!(!volume.connection_state().is_some_and(ConnectionState::is_live));

    volume.set_auto_reconnect(true);

    cmdr_fs::testing::wait_until_async(
        Duration::from_secs(6),
        "❗ turning the switch on to act now, not at the next drop",
        || volume.connection_state().is_some_and(ConnectionState::is_live),
    )
    .await;
    assert!(volume.exists(&hello).await, "the rebuilt client serves the root");
}
