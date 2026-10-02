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
use crate::multipart::{MAX_PARTS, MIN_PART_SIZE, ShortTail, plan_parts_with_floor};
use crate::profile::{ConditionalOp, NoOverwrite};

/// The billed work of one planned operation on one provider.
#[derive(Debug, Clone, PartialEq)]
pub struct Workload {
    /// The price table's key for the provider; `None` for "Other", which has
    /// no list prices.
    pub(crate) price_key: Option<&'static str>,
    /// Whether a no-overwrite PUT, `CopyObject`, and multipart completion
    /// HEAD their key first, because the provider ignores or lacks a header
    /// for that write (`ProviderProfile::no_overwrite`).
    pub(crate) checks: Checks,
    /// How the provider's multipart plans cut a short tail.
    pub(crate) short_tail: ShortTail,
    /// Whether a one-PUT overwrite of an existing object lands through a temp
    /// key, because the provider is off the `refuses_short_body` allowlist
    /// (`volume/temp_overwrite.rs`).
    pub(crate) overwrites_through_temp: bool,
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
        let profile = provider.profile();
        Self {
            price_key: match provider {
                S3Provider::Other { .. } => None,
                priced => Some(priced.kind_name()),
            },
            checks: Checks::of(provider),
            short_tail: profile.as_ref().map_or(ShortTail::Fold, |profile| profile.short_tail),
            // A profile that won't build is the temp-key side, the safe one.
            overwrites_through_temp: !profile.as_ref().is_ok_and(|profile| profile.refuses_short_body),
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
        let whole = fits_one_put(size);
        let parts = self.part_count(size);
        if whole {
            self.add(RequestKind::PutObject, 1);
        } else {
            self.add(RequestKind::CreateMultipartUpload, 1);
            self.add(RequestKind::UploadPart, parts);
            self.add(RequestKind::CompleteMultipartUpload, 1);
        }
        let checks = if whole { self.checks.put } else { self.checks.complete };
        self.add(RequestKind::HeadObject, 1 + checks);
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
            self.add(RequestKind::UploadPartCopy, self.part_count(size));
            self.add(RequestKind::CompleteMultipartUpload, 1);
        }
        let checks = if whole { self.checks.copy } else { self.checks.complete };
        self.add(RequestKind::HeadObject, 2 + checks);
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

    /// An existing object the operation writes over. `modified_at` is its
    /// upload time, when known: a provider with a minimum storage duration
    /// bills its remaining days, as for a delete, though no delete is sent.
    pub fn replace_object(&mut self, size: u64, modified_at: Option<u64>) {
        if let Some(modified_at) = modified_at {
            let age_days = self.now.saturating_sub(modified_at) / SECONDS_PER_DAY;
            self.dated_deletions.push((size, age_days));
        }
    }

    /// What an [`upload`](Self::upload) of `size` adds when it lands on an
    /// EXISTING key. Off the `refuses_short_body` allowlist a one-PUT
    /// overwrite goes through a temp key (`volume/temp_overwrite.rs`): a HEAD
    /// finding the original, the temp's HEAD, a `CopyObject` onto the final key
    /// and its verifying HEAD, then a HEAD of the temp's token and its delete.
    /// The temp goes brand new, so a minimum storage duration bills its whole
    /// term. A multipart upload goes straight to the key (only its completion
    /// publishes), and adds nothing.
    pub fn upload_over(&mut self, size: u64) {
        if !self.overwrites_through_temp || !fits_one_put(size) {
            return;
        }
        self.add(RequestKind::HeadObject, 4);
        self.add(RequestKind::CopyObject, 1);
        self.add(RequestKind::DeleteObject, 1);
        self.dated_deletions.push((size, 0));
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

    /// How many parts an object of `size` goes in: the write paths' own plan.
    /// Past S3's 48.8 TiB ceiling the write would refuse; the estimate counts
    /// the most parts it could have sent.
    fn part_count(&self, size: u64) -> u64 {
        plan_parts_with_floor(size, MIN_PART_SIZE, self.short_tail).map_or(MAX_PARTS, |plan| u64::from(plan.part_count))
    }
}

/// The no-overwrite HEADs each write sends where the provider can't refuse
/// on its own: one before a PUT or a `CopyObject`, and for a multipart write
/// one before it starts and again before its completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Checks {
    pub put: u64,
    pub copy: u64,
    pub complete: u64,
}

impl Checks {
    fn of(provider: &S3Provider) -> Self {
        let Ok(profile) = provider.profile() else {
            return Self {
                put: 1,
                copy: 1,
                complete: 2,
            };
        };
        let heads = |op, count| match profile.no_overwrite(op) {
            NoOverwrite::CheckThenWrite => count,
            NoOverwrite::IfNoneMatch | NoOverwrite::CloudflareCopyHeader => 0,
        };
        Self {
            put: heads(ConditionalOp::Put, 1),
            copy: heads(ConditionalOp::Copy, 1),
            complete: heads(ConditionalOp::CompleteMultipart, 2),
        }
    }
}

const SECONDS_PER_DAY: u64 = 86_400;

/// Whether an upload of `size` goes as one PUT (`volume/writes.rs`'s
/// `shape_for`): one part once a short tail is folded in.
fn fits_one_put(size: u64) -> bool {
    plan_parts_with_floor(size, MIN_PART_SIZE, ShortTail::Fold).is_ok_and(|plan| plan.part_count <= 1)
}
