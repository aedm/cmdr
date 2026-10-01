//! Overwrites through a temp key, for a provider not trusted to refuse a
//! short body.
//!
//! ❗ S3 publishes nothing short of a PUT's `Content-Length`, but not every
//! server keeps that promise: VersityGW stores whatever arrived before the
//! connection dropped (`apps/desktop/test/s3-servers/README.md`). Written in
//! place, an overwrite cancelled or cut off there has already replaced the
//! original with a truncated object. So on a provider off the
//! `refuses_short_body` allowlist (`profile.rs`), an overwrite of an EXISTING
//! object:
//!
//! 1. records a temp key in the ledger (`<name>.cmdr-tmp-<uuid>`, hidden from
//!    the pane while in flight, `cmdr_fs::staging`), then writes the new bytes
//!    there under `CreateNew`, with the write's own token;
//! 2. verifies the temp, and honours a Cancel right before the copy, which is
//!    what publishes;
//! 3. copies the temp onto the final key on the server (`server_copy.rs`), a
//!    request with no body to cut short;
//! 4. deletes the temp, only while it carries this write's token, and forgets
//!    the record once the server confirms it gone.
//!
//! A crash at any step leaves the original whole: before step 3 it was never
//! touched, and the copy itself replaces it in one go. What a crash leaves is
//! the temp, which the next connect's sweep removes by its token.
//!
//! A write to a free name, and a multipart upload (only its completion
//! publishes), go straight to the key: there's no original to lose.

use std::ops::ControlFlow;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use cmdr_fs::staging::StagingTemp;
use cmdr_fs::volume::{ServerCopyProgress, VolumeError, VolumeReadStream, WriteMode};
use log::{debug, warn};

use super::paths::{Target, target_of};
use super::server_copy::{CopyFrom, SourceObject};
use super::upload_ledger::TempObject;
use super::writes::{Progress, UploadShape, WriteTarget};
use super::{S3Volume, S3VolumeInner};
use crate::metadata::WRITE_TOKEN_HEADER;
use crate::transport::S3Client;

/// The copy onto the final key reports through the write's own progress: its
/// bytes were counted as the temp filled, so all it can still say is "stop".
struct LandingProgress<'a> {
    progress: &'a Progress<'a>,
    size: u64,
}

impl ServerCopyProgress for LandingProgress<'_> {
    fn advanced(&self, _done: u64, _total: u64) -> ControlFlow<()> {
        self.progress.at(self.size)
    }

    fn checkpoint(&self) -> Pin<Box<dyn Future<Output = ControlFlow<()>> + Send + '_>> {
        Box::pin(async { ControlFlow::Continue(()) })
    }
}

impl S3Volume {
    /// Writes `stream` to a temp key beside `target`, then copies it over
    /// `target` on the server. Returns the bytes written.
    pub(super) async fn overwrite_through_temp(
        &self,
        client: &Arc<S3Client>,
        dest: &Path,
        target: &WriteTarget<'_>,
        shape: UploadShape,
        stream: Box<dyn VolumeReadStream>,
        progress: &Progress<'_>,
    ) -> Result<u64, VolumeError> {
        // The guard keeps the temp out of the pane while this write owns it.
        let temp = StagingTemp::mint(dest, None);
        let temp_remote = self.to_remote_path(temp.path())?;
        let Target::Key {
            bucket: temp_bucket,
            key: temp_key,
        } = target_of(&temp_remote)
        else {
            return Err(VolumeError::IsADirectory(temp_remote));
        };
        let token = target
            .metadata
            .write_token
            .clone()
            .unwrap_or_else(crate::metadata::write_token);
        let mut metadata = target.metadata.clone();
        metadata.write_token = Some(token.clone());
        let record = TempObject {
            account: self.inner.account(),
            bucket: temp_bucket.to_string(),
            key: temp_key.to_string(),
            token,
        };
        // ❗ Recorded before the first byte: a crash mid-PUT on a server that
        // keeps a cut-off body still leaves something the sweep can find.
        self.inner.ledger.temp_started(&record);
        let temp_target = WriteTarget {
            bucket: temp_bucket,
            key: temp_key,
            remote: &temp_remote,
            mode: WriteMode::CreateNew,
            metadata: &metadata,
        };
        debug!(target: "volume", "s3 overwrite of {} goes through {temp_remote}", target.remote);
        let written = match shape {
            UploadShape::Single(size) => self.put_streamed(client, &temp_target, size, stream, progress).await,
            UploadShape::Open => self.upload_in_parts(client, &temp_target, None, stream, progress).await,
            UploadShape::Parts(plan) => {
                self.upload_in_parts(client, &temp_target, Some(plan), stream, progress)
                    .await
            }
        };
        let outcome = match written {
            Ok(size) => self.land_temp(client, &temp_target, target, size, progress).await,
            Err(e) => Err(e),
        };
        self.inner.remove_temp(client, &record).await;
        drop(temp);
        outcome
    }

    /// The temp is whole and verified: copy it over the final key, unless a
    /// Cancel came in while it filled.
    async fn land_temp(
        &self,
        client: &Arc<S3Client>,
        temp: &WriteTarget<'_>,
        target: &WriteTarget<'_>,
        size: u64,
        progress: &Progress<'_>,
    ) -> Result<u64, VolumeError> {
        // ❗ The copy is what publishes: a Cancel lands here at the latest.
        if progress.at(size).is_break() {
            return Err(VolumeError::Cancelled(self.volume_id().to_string()));
        }
        let head = self
            .head_object(client, temp.bucket, temp.key, temp.remote)
            .await?
            .ok_or_else(|| VolumeError::IoError {
                message: format!("{}: the temp was gone right after it was written", temp.remote),
                raw_os_error: None,
            })?;
        let object = SourceObject::from_head(&head);
        let from = CopyFrom {
            bucket: temp.bucket,
            key: temp.key,
            object: &object,
        };
        let landing = LandingProgress { progress, size };
        self.copy_key(
            client,
            &from,
            target.bucket,
            target.key,
            target.remote,
            WriteMode::CreateOrReplace,
            &landing,
        )
        .await
    }
}

impl S3VolumeInner {
    /// Deletes a temp object while it still carries its write's token, and
    /// forgets its record once the server confirms it gone. Anything else at
    /// the key is never ours to delete; an answer that doesn't come keeps the
    /// record for the next sweep. Answers whether the record settled.
    pub(super) async fn remove_temp(&self, client: &S3Client, record: &TempObject) -> bool {
        let remote = format!("/{}/{}", record.bucket, record.key);
        let Ok(head) = crate::ops::head_object(client.profile(), &record.bucket, &record.key) else {
            self.ledger.temp_finished(record);
            return true;
        };
        let answer = match client.exchange(head, crate::transport::QUERY_BUDGET).await {
            Ok(answer) => answer,
            Err(e) => {
                debug!(target: "volume", "s3: checking the temp {remote} didn't reach the server: {e}");
                self.ledger.temp_abandoned(record);
                return false;
            }
        };
        if answer.status == http::StatusCode::NOT_FOUND {
            self.ledger.temp_finished(record);
            return true;
        }
        if !answer.status.is_success() {
            self.ledger.temp_abandoned(record);
            return false;
        }
        if answer.header(WRITE_TOKEN_HEADER) != Some(record.token.as_str()) {
            // Not this write's object: never ours to remove.
            self.ledger.temp_finished(record);
            return true;
        }
        let deleted = match crate::ops::delete_object(client.profile(), &record.bucket, &record.key) {
            Ok(request) => client
                .exchange(request, crate::transport::QUERY_BUDGET)
                .await
                .is_ok_and(|answer| answer.status.is_success() || answer.status == http::StatusCode::NOT_FOUND),
            Err(_) => false,
        };
        if deleted {
            self.ledger.temp_finished(record);
        } else {
            warn!(target: "volume", "s3: couldn't remove the temp {remote}; the next connect tries again");
            self.ledger.temp_abandoned(record);
        }
        deleted
    }
}
