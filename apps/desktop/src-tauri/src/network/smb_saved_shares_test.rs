//! How a saved share's mount answers read to the servers family.

use super::*;

fn outcome(error: MountError) -> String {
    format!("{:?}", outcome_of(&error))
}

/// ❗ **A guest turned away is asked for a password, ❌ never told its password
/// was wrong**: nobody offered one. An account the share turns away reads as a
/// rejection, since the sheet's answer is another password for the same place.
#[test]
fn a_refused_mount_asks_the_right_question() {
    let (server, share) = ("nas".to_string(), "photos".to_string());
    assert_eq!(
        outcome(MountError::AuthRequired {
            server: server.clone(),
            share: share.clone()
        }),
        "NeedsCredentials"
    );
    assert_eq!(
        outcome(MountError::AuthFailed { server: server.clone() }),
        "AuthenticationRejected"
    );
    assert_eq!(
        outcome(MountError::PermissionDenied {
            server: server.clone(),
            share: share.clone(),
            username: "sven".to_string()
        }),
        "AuthenticationRejected"
    );
    assert_eq!(outcome(MountError::Timeout { server: server.clone() }), "TimedOut");
    assert_eq!(outcome(MountError::Cancelled { share: share.clone() }), "Cancelled");
    for about_the_share in [
        MountError::HostUnreachable { server: server.clone() },
        MountError::ShareNotFound {
            server: server.clone(),
            share: share.clone(),
        },
        MountError::MountMissing { server, share },
        MountError::GvfsMissing,
    ] {
        assert_eq!(outcome(about_the_share), "Unreachable");
    }
}

/// A row no mount went through has no address to dial, and says so without
/// dialing anything.
#[tokio::test]
async fn a_share_that_never_mounted_has_nothing_to_dial() {
    let row = KnownNetworkShare {
        server_name: "nas".to_string(),
        share_name: "photos".to_string(),
        protocol: "smb".to_string(),
        last_connected_at: String::new(),
        last_connection_mode: ConnectionMode::Guest,
        last_known_auth_options: AuthOptions::GuestOrCredentials,
        username: None,
        address: None,
        port: None,
        volume_id: None,
        mount_path: None,
        pinned: false,
    };
    let answer = connect_saved_share(row, "never-mounted", None, None).await;
    assert!(matches!(answer, ServerConnectOutcome::Unreachable), "got {answer:?}");
}
