//! The whole `impl Volume`: the capability answers, and one-line delegators to
//! the modules that do the work. Every answer here is deliberate: a default
//! this backend accepts silently is a promise it may not be able to keep.
//!
//! ❗ **Read-only for now, and it says so.** Listing and stat work; reads,
//! writes, and copies are later milestones (`docs/specs/s3-support-plan.md`),
//! so `is_writable`, `supports_export`, and `supports_streaming` answer
//! `false` and every mutation keeps the trait's `NotSupported` default. ❌ No
//! `true` here before the method it speaks for works: `is_writable` is button
//! state, and `supports_export` gates copy-from.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use cmdr_fs::entry::FileEntry;
use cmdr_fs::volume::{
    BackendKind, LaneKey, ListingProgress, Retirement, SignInShape, SpaceInfo, Volume, VolumeError, WatchCoverage,
};
use tokio_util::sync::CancellationToken;

use super::{BACKEND, S3Volume};

impl S3Volume {
    /// Runs `work`, noticing on the way out if the answer says the server is
    /// gone. ❗ Every delegator below that can reach the wire wraps itself in
    /// this: with no watcher, the operations ARE the detector (`reconnect.rs`).
    ///
    /// It also counts `work` as waiting on the server, which the silence watch
    /// looks after (`cmdr_fs::volume::liveness`), and cuts it with
    /// `DeviceDisconnected` the moment the client is found gone: a silent
    /// server closes nothing, so nothing else would ever end the wait.
    async fn noting<T>(&self, work: impl Future<Output = Result<T, VolumeError>> + Send) -> Result<T, VolumeError> {
        let client = self.inner.client.read().await.clone();
        let outcome = match client {
            // No client: the work answers `DeviceDisconnected` on its own, at once.
            None => work.await,
            Some(client) => {
                let liveness = Arc::clone(client.liveness());
                let waiting = liveness.begin();
                if waiting.needs_a_watch() {
                    self.inner.watch_over(&client);
                }
                drop(client);
                let outcome = tokio::select! {
                    biased;
                    outcome = work => outcome,
                    () = liveness.lost().cancelled() => Err(VolumeError::DeviceDisconnected(self.inner.volume_id.clone())),
                };
                drop(waiting);
                outcome
            }
        };
        if let Err(error) = &outcome {
            self.note_lost_session(error);
        }
        outcome
    }
}

impl Volume for S3Volume {
    fn name(&self) -> &str {
        &self.name
    }

    fn root(&self) -> &Path {
        self.root.app_root()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    /// The ACCOUNT, not the place: every bucket under one key shares the
    /// endpoint's request budget (Hetzner's 750 requests/s per bucket is the
    /// low bar, and a throttle answers per account on most providers).
    fn lane_key(&self) -> LaneKey {
        let params = self.inner.params();
        LaneKey::new(format!(
            "s3:{}:{}:{}",
            params.host(),
            params.port(),
            params.access_key_id()
        ))
    }

    fn max_concurrent_ops(&self) -> usize {
        self.inner.host.settings().max_concurrent_operations(BACKEND)
    }

    fn list_directory<'a>(
        &'a self,
        path: &'a Path,
        on_progress: Option<&'a (dyn Fn(ListingProgress) + Sync)>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<FileEntry>, VolumeError>> + Send + 'a>> {
        Box::pin(self.noting(self.list_directory_impl(path, on_progress, None)))
    }

    fn list_directory_with_cancel<'a>(
        &'a self,
        path: &'a Path,
        on_progress: Option<&'a (dyn Fn(ListingProgress) + Sync)>,
        cancel: Option<&'a CancellationToken>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<FileEntry>, VolumeError>> + Send + 'a>> {
        Box::pin(self.noting(self.list_directory_impl(path, on_progress, cancel)))
    }

    fn get_metadata<'a>(
        &'a self,
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<FileEntry, VolumeError>> + Send + 'a>> {
        Box::pin(self.noting(self.get_metadata_impl(path)))
    }

    /// Kept rather than taken from the trait default: the default routes
    /// through `noting`, so a bare existence probe on a dropped link would
    /// report a connection transition and drive a reconnect. An `exists`
    /// question stays a question.
    fn exists<'a>(&'a self, path: &'a Path) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move { self.get_metadata_impl(path).await.is_ok() })
    }

    // ── What it can't do yet, said out loud ──────────────────────────

    /// Reads arrive with the read milestone.
    fn supports_streaming(&self) -> bool {
        false
    }

    /// Copy-from arrives with the read milestone; until then the guard refuses
    /// a copy off S3 before it opens anything.
    fn supports_export(&self) -> bool {
        false
    }

    /// Writes arrive with the write milestone; until then New folder, New
    /// file, Rename, and Paste render disabled here.
    fn is_writable(&self) -> bool {
        false
    }

    // ── Lifecycle ────────────────────────────────────────────────────

    fn retirement(&self) -> Option<&Retirement> {
        Some(&self.inner.retirement)
    }

    /// Retires this instance ❗ without touching the live client: whoever
    /// still holds an `Arc` keeps using it.
    fn on_superseded(&self) {
        self.inner.retirement.retire();
    }

    /// The volume is leaving the registry. ❌ No connection event: the
    /// frontend learns through `volumes-changed`.
    fn on_unmount(&self) {
        self.inner.unmounted.store(true, Ordering::Relaxed);
        self.inner.mark_gone_silently();
        let inner = Arc::clone(&self.inner);
        inner.host.runtime().clone().spawn(async move {
            inner.client.write().await.take();
        });
    }

    /// Where this connection stands, for the switcher dot and the pane's
    /// connect views. `Direct` means the last request that reached the wire
    /// came back.
    fn connection_state(&self) -> Option<cmdr_fs::volume::ConnectionState> {
        use super::ConnectionState;
        use cmdr_fs::volume::ConnectionState as Published;
        Some(match self.inner.connection_state() {
            ConnectionState::Connected => Published::Direct,
            ConnectionState::Disconnected => Published::Disconnected,
            ConnectionState::NeedsCredentials => Published::NeedsSignIn,
        })
    }

    fn backend_kind(&self) -> BackendKind {
        BackendKind::S3
    }

    /// One rung, one prompt: the secret access key under its key id.
    fn sign_in_prompt(&self) -> SignInShape {
        SignInShape::AccessKeys
    }

    fn attempt_reconnect<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<(), VolumeError>> + Send + 'a>> {
        Box::pin(self.inner.do_attempt_reconnect())
    }

    /// `username` is the access key id, which must be this volume's own.
    fn reconnect_with_credentials<'a>(
        &'a self,
        username: String,
        password: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), VolumeError>> + Send + 'a>> {
        Box::pin(self.inner.do_reconnect_with_credentials(username, password))
    }

    // ── What this backend is, in capability terms ────────────────────

    fn supports_local_fs_access(&self) -> bool {
        false
    }

    fn paths_are_os_visible(&self) -> bool {
        false
    }

    fn local_path(&self) -> Option<PathBuf> {
        None
    }

    fn operations_are_local(&self) -> bool {
        false
    }

    /// ❗ No watcher (S3 has no change feed a client can hold), so ❌ nothing
    /// here may call `authoritative_listing`.
    fn can_watch_listings(&self) -> bool {
        false
    }

    fn listing_watch_coverage(&self, _path: &Path) -> WatchCoverage {
        WatchCoverage::None
    }

    /// S3 has no capacity and no cheap "bytes used": summing a bucket is a
    /// listing of every key in it, billed per thousand.
    fn get_space_info<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<SpaceInfo, VolumeError>> + Send + 'a>> {
        Box::pin(async { Err(VolumeError::NotSupported) })
    }

    /// Nothing to poll: the answer is `NotSupported` every time.
    fn space_poll_interval(&self) -> Option<Duration> {
        None
    }
}
