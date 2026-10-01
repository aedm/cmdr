//! How a big object is cut into parts.
//!
//! One part size per upload, every part that size except the last: R2 refuses
//! a completion whose parts differ (`InvalidPart`), so we always do it, and it
//! makes a resumed or re-sent part land on the same byte range. A tail under
//! 5 MiB folds into the part before it, because Garage refuses an
//! `UploadPartCopy` source that small even as the last part
//! (`apps/desktop/test/s3-servers/README.md`). Whether R2 accepts a last part
//! LARGER than the rest is for M8 to confirm on a real bucket.

/// 1 MiB.
const MIB: u64 = 1024 * 1024;

/// The smallest part we cut. Well above S3's 5 MiB floor: fewer, bigger parts
/// mean fewer billed requests, and progress still moves every few seconds.
pub(crate) const MIN_PART_SIZE: u64 = 64 * MIB;

/// A last part smaller than this joins the part before it.
const MIN_TAIL: u64 = 5 * MIB;

/// S3's ceiling on parts per upload, on every provider we know.
pub(crate) const MAX_PARTS: u64 = 10_000;

/// S3's ceiling on one part.
pub(crate) const MAX_PART_SIZE: u64 = 5 * 1024 * MIB;

/// An upload's parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PartPlan {
    /// Every part's size except the last, which may be smaller, or up to
    /// 5 MiB larger when a short tail folded into it.
    pub part_size: u64,
    pub part_count: u32,
    pub total: u64,
}

/// Too big for 10,000 parts of 5 GiB (about 48.8 TiB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TooLarge;

/// The plan for `total` bytes: parts of at least 64 MiB and at least
/// `total / 10,000`, rounded up to a whole MiB, with a tail under 5 MiB folded
/// into the part before it unless that part would pass 5 GiB. Production
/// plans through [`plan_parts_with_floor`] with the volume's floor, which is
/// this unless a Docker cell lowered it.
#[cfg(test)]
pub(crate) fn plan_parts(total: u64) -> Result<PartPlan, TooLarge> {
    plan_parts_with_floor(total, MIN_PART_SIZE)
}

/// [`plan_parts`] with a smaller floor than 64 MiB, for a Docker cell that
/// wants several parts without uploading hundreds of megabytes. ❗ Production
/// always plans with [`MIN_PART_SIZE`]; `floor` is clamped to S3's 5 MiB.
pub(crate) fn plan_parts_with_floor(total: u64, floor: u64) -> Result<PartPlan, TooLarge> {
    if total > MAX_PARTS * MAX_PART_SIZE {
        return Err(TooLarge);
    }
    // At most 5 GiB, itself a whole MiB, so rounding can't push it past the cap.
    let needed = total.div_ceil(MAX_PARTS).div_ceil(MIB) * MIB;
    let part_size = needed.max(floor.max(MIN_TAIL));
    let mut part_count = total.div_ceil(part_size).max(1);
    let tail = total % part_size;
    if part_count > 1 && tail > 0 && tail < MIN_TAIL && part_size + tail <= MAX_PART_SIZE {
        part_count -= 1;
    }
    Ok(PartPlan {
        part_size,
        part_count: u32::try_from(part_count).map_err(|_| TooLarge)?,
        total,
    })
}

impl PartPlan {
    /// The inclusive byte range of 1-based `part_number`, the shape
    /// `x-amz-copy-source-range` and a ranged read both take. The last part
    /// runs to the end of the object. Meaningless for an empty upload, which
    /// has no bytes to range over.
    pub(crate) fn range(&self, part_number: u32) -> (u64, u64) {
        let start = u64::from(part_number.saturating_sub(1)) * self.part_size;
        let end = if part_number >= self.part_count {
            self.total
        } else {
            (start + self.part_size).min(self.total)
        };
        (start, end.saturating_sub(1))
    }
}

#[cfg(test)]
#[path = "multipart_test.rs"]
mod multipart_test;
