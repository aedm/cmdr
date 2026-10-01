//! What one planned operation sends to one provider, counted the way this
//! backend sends it: the requests by kind, the bytes downloaded to the Mac, and
//! the objects deleted with their ages.
//!
//! ❗ Each method mirrors a write path, so a change to how many requests a
//! write sends (a new verifying HEAD, a different part floor) changes the
//! matching method here too. The request shapes are listed per method.

use std::collections::HashMap;

use super::prices::RequestKind;
use crate::S3Provider;
use crate::multipart::{MAX_PARTS, MIN_PART_SIZE, plan_parts_with_floor};

/// The billed work of one planned operation on one provider.
#[derive(Debug, Clone, PartialEq)]
pub struct Workload {
    /// The price table's key for the provider; `None` for "Other", which has
    /// no list prices.
    pub(crate) price_key: Option<&'static str>,
    /// Whether a write HEADs its key first, because the provider ignores or
    /// lacks `If-None-Match` (`DETAILS.md` § "No-overwrite writes").
    pub(crate) checks_before_write: bool,
    pub(crate) requests: HashMap<RequestKind, u64>,
    /// Objects deleted, batched into `DeleteObjects` at estimate time.
    pub(crate) deleted_objects: u64,
    pub(crate) egress_bytes: u64,
    /// Every deleted object whose age is known: (size, whole days old).
    pub(crate) dated_deletions: Vec<(u64, u64)>,
    /// Unix seconds the ages are measured from.
    pub(crate) now: u64,
}

impl Workload {
    /// An empty workload for `provider`, with ages measured from now.
    pub fn for_provider(provider: &S3Provider) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        Self::for_provider_at(provider, now)
    }

    /// An empty workload for `provider`, with ages measured from `now` (Unix
    /// seconds).
    pub fn for_provider_at(provider: &S3Provider, now: u64) -> Self {
        Self {
            price_key: match provider {
                S3Provider::Other { .. } => None,
                priced => Some(priced.kind_name()),
            },
            // AWS takes `If-None-Match` on every write and R2 on a PUT and a
            // copy (`ProviderProfile::from_preset`); everyone else HEADs first.
            checks_before_write: !matches!(provider, S3Provider::Aws { .. } | S3Provider::R2 { .. }),
            requests: HashMap::new(),
            deleted_objects: 0,
            egress_bytes: 0,
            dated_deletions: Vec::new(),
            now,
        }
    }

    /// One file uploaded (`volume/writes.rs`): one PUT up to the part floor,
    /// else Create, a PUT per part, and Complete; a verifying HEAD after, and
    /// the no-overwrite HEADs where the provider can't refuse on its own.
    pub fn upload(&mut self, size: u64) {
        let parts = part_count(size);
        let whole = parts <= 1;
        if whole {
            self.add(RequestKind::PutObject, 1);
        } else {
            self.add(RequestKind::CreateMultipartUpload, 1);
            self.add(RequestKind::UploadPart, parts);
            self.add(RequestKind::CompleteMultipartUpload, 1);
        }
        self.add(RequestKind::HeadObject, 1 + self.checks_for(whole));
    }

    /// One object downloaded to the Mac (`volume/streams.rs`): one GET, and its
    /// bytes leave the provider.
    pub fn download(&mut self, size: u64) {
        self.add(RequestKind::GetObject, 1);
        self.egress_bytes += size;
    }

    /// One object copied within the account, without its bytes leaving
    /// (`volume/server_copy.rs`): a HEAD of the source, one `CopyObject` up to
    /// the part floor, else Create, an `UploadPartCopy` per part, and Complete;
    /// then the verifying HEAD, and the no-overwrite HEADs where needed.
    pub fn copy_on_server(&mut self, size: u64) {
        // `copies_whole` is `size <= part floor`, which differs from the
        // upload's one-part plan just past the floor, where a small tail folds.
        let whole = size <= MIN_PART_SIZE;
        if whole {
            self.add(RequestKind::CopyObject, 1);
        } else {
            self.add(RequestKind::CreateMultipartUpload, 1);
            self.add(RequestKind::UploadPartCopy, part_count(size));
            self.add(RequestKind::CompleteMultipartUpload, 1);
        }
        self.add(RequestKind::HeadObject, 2 + self.checks_for(whole));
    }

    /// One object deleted, in a `DeleteObjects` batch (`volume/batch.rs`).
    /// `modified_at` is its upload time in Unix seconds (the listing's
    /// `LastModified`), when known: a provider with a minimum storage duration
    /// bills a young object's remaining days.
    pub fn delete_object(&mut self, size: u64, modified_at: Option<u64>) {
        self.deleted_objects += 1;
        if let Some(modified_at) = modified_at {
            // A date in the future (clock skew) counts as brand new.
            let age_days = self.now.saturating_sub(modified_at) / SECONDS_PER_DAY;
            self.dated_deletions.push((size, age_days));
        }
    }

    /// One folder removed once it's empty (`volume/mutation.rs`): a listing
    /// capped at two keys, then the marker's delete.
    pub fn delete_folder(&mut self) {
        self.add(RequestKind::ListObjectsV2, 1);
        self.add(RequestKind::DeleteObject, 1);
    }

    /// One folder listed while the operation walks a tree it didn't scan
    /// first: a page per thousand keys, counted as one.
    pub fn list_folder(&mut self) {
        self.add(RequestKind::ListObjectsV2, 1);
    }

    fn add(&mut self, kind: RequestKind, count: u64) {
        *self.requests.entry(kind).or_default() += count;
    }

    /// The no-overwrite HEADs a write sends where the provider can't refuse on
    /// its own: one before a PUT, and again before a Complete.
    fn checks_for(&self, whole: bool) -> u64 {
        match (self.checks_before_write, whole) {
            (false, _) => 0,
            (true, true) => 1,
            (true, false) => 2,
        }
    }
}

const SECONDS_PER_DAY: u64 = 86_400;

/// How many parts an object of `size` goes in: the write paths' own plan. Past
/// S3's 48.8 TiB ceiling the write would refuse; the estimate counts the most
/// parts it could have sent.
fn part_count(size: u64) -> u64 {
    plan_parts_with_floor(size, MIN_PART_SIZE).map_or(MAX_PARTS, |plan| u64::from(plan.part_count))
}
