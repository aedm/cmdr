//! The liveness mapping, and the handle it reads through surviving a client
//! rebuild. The rebuild itself, against a real server, is
//! `session_integration_test::smb_integration_reconnect_repoints_the_liveness_reading`.

use super::*;
use smb2::transport::MockTransport;
use std::sync::Arc;
use std::time::Duration;

/// A connection over a mock transport: no server, no Docker, and its own clocks.
fn mock_connection() -> Connection {
    let transport = Arc::new(MockTransport::new());
    Connection::from_transport(Box::new(Arc::clone(&transport)), Box::new(transport), "mock-server")
}

/// Only the reading `Error::ServerUnresponsive` rests on may become `Dead`. That
/// verdict is half of what lets the watchdog end a wait, so a `Quiet` link (which
/// proves nothing) or an `Idle` one must never be promoted to it.
#[test]
fn only_an_unresponsive_link_reads_dead() {
    let silent_for = Duration::from_secs(20);
    assert_eq!(
        verdict_for(Liveness::Unresponsive { silent_for }),
        Some(ConnectionLiveness::Dead)
    );
    assert_eq!(verdict_for(Liveness::Alive), Some(ConnectionLiveness::Alive));
    assert_eq!(
        verdict_for(Liveness::Quiet { silent_for }),
        None,
        "quiet proves nothing"
    );
    assert_eq!(
        verdict_for(Liveness::Idle),
        None,
        "nothing outstanding to be silent about"
    );
    assert_eq!(
        verdict_for(Liveness::Disconnected),
        None,
        "a torn-down connection has already failed its waiters; the retry owns that"
    );
}

/// After a rebuild the reading has to come from the NEW client's connection. The
/// old one is torn down, and a handle left pointing at it would answer for a
/// session nobody uses any more.
#[tokio::test]
async fn a_replaced_connection_answers_for_the_new_session() {
    let old = mock_connection();
    let live = LiveConnection::new(Some(old.clone()));
    let new = mock_connection();

    live.replace(Some(new));
    old.mark_dead();

    let current = live.current().expect("the new connection is installed");
    assert_eq!(old.liveness(), Liveness::Disconnected, "the old session is gone");
    assert_ne!(
        current.liveness(),
        Liveness::Disconnected,
        "the reading must follow the client that's installed now"
    );
}

/// With no client there is nothing to ask, so both readings say so rather than
/// inventing a number.
#[tokio::test]
async fn no_client_means_no_reading() {
    let live = LiveConnection::new(Some(mock_connection()));
    assert_eq!(
        live.bytes_received(),
        Some(0),
        "a fresh connection has received nothing yet"
    );

    live.replace(None);
    assert_eq!(live.verdict(), None);
    assert_eq!(live.bytes_received(), None);
}
