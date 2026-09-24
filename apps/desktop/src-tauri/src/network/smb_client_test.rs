//! How the share listing treats a host the person means to sign in to.

use super::*;
use crate::network::smb_cache::cache_shares;

fn listing(auth_mode: AuthMode) -> ShareListResult {
    ShareListResult {
        shares: Vec::new(),
        auth_mode,
        from_cache: false,
    }
}

/// ❗ **A host someone typed an account for never answers with a guest
/// listing.** On a `map to guest = bad user` Samba a guest "succeeds" with an
/// almost-empty list, and the person has already said they want to sign in. So
/// a guest listing in the cache doesn't count, and nothing is dialed as guest:
/// the answer is "sign in", which the frontend takes to the Keychain and then
/// the sheet. `host.invalid` proves nothing was dialed, since reaching it would
/// be a `HostUnreachable`.
#[tokio::test]
async fn a_host_that_wants_an_account_asks_for_one_instead_of_listing_as_guest() {
    cache_shares("wants-account-guest-cached", &listing(AuthMode::GuestAllowed), 60_000);

    let answer = list_shares(
        "wants-account-guest-cached",
        "host.invalid",
        None,
        445,
        None,
        GuestAttempt::Skip,
        Some(2_000),
        Some(0),
    )
    .await;

    assert!(
        matches!(answer, Err(ShareListError::AuthRequired { .. })),
        "got {answer:?}"
    );
}

/// An account's own listing in the cache still answers: it is exactly what the
/// person asked for, and re-dialing it on every open would be wasted round-trips.
#[tokio::test]
async fn an_account_listing_in_the_cache_still_answers_a_host_that_wants_one() {
    cache_shares("wants-account-creds-cached", &listing(AuthMode::CredsRequired), 60_000);

    let answer = list_shares(
        "wants-account-creds-cached",
        "host.invalid",
        None,
        445,
        None,
        GuestAttempt::Skip,
        Some(2_000),
        Some(0),
    )
    .await
    .expect("the cached account listing");

    assert!(answer.from_cache);
    assert_eq!(answer.auth_mode, AuthMode::CredsRequired);
}
