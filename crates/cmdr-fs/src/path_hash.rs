//! A fixed 64-bit hash of a folder path, for maps that keep the hash and drop the path.
//!
//! Two long-lived maps are keyed on folder paths they only ever LOOK UP, never enumerate:
//! search's importance weights (`search::ranking::ImportanceWeights` in the app) and the
//! media coverage score cache (`cmdr-index`'s `media_index::coverage::scores`). Hundreds of
//! thousands of absolute paths averaging over 100 bytes are dead weight once hashed, so
//! both store [`hash_path`] in their place and use [`PrehashedState`] to skip hashing the
//! hash again. Each map's own docs weigh what a collision costs it; this module only
//! promises the hash is fixed, well mixed, and cheap to stream.

use std::hash::{BuildHasher, Hasher};

/// The FNV-1a 64-bit offset basis and prime.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Hash a folder path to the 64-bit key a map stores in place of the path.
///
/// FNV-1a over the bytes, then a splitmix64 finalizer. The finalizer is not optional:
/// raw FNV-1a barely mixes its LOW bits, and that's exactly where hashbrown takes its
/// bucket index, so paths sharing a suffix would pile into the same buckets.
///
/// Fixed and fully specified on purpose, rather than `RandomState` or any hasher whose
/// output can move under us: a given path hashes to the same value in every run and
/// every build, so the mapping is a testable property instead of an implementation
/// detail. Nothing persists a hash, so this is free to change — a different function
/// just yields a different, equally consistent mapping.
pub fn hash_path(path: &str) -> u64 {
    let mut hasher = PathHasher::new();
    hasher.write(path.as_bytes());
    hasher.finish()
}

/// [`hash_path`] fed one piece at a time, so a caller that can produce a path's bytes
/// in order without owning the whole string doesn't have to build one.
///
/// Search's ranking hot path walks the index's parent chain to get a folder's path, and
/// the only thing it does with that path is hash it. Feeding the components straight in
/// keeps a broad query (millions of matches) from allocating a `String` per candidate.
/// Byte-for-byte identical to `hash_path` of the joined path (pinned app-side by
/// `streamed_hash_matches_whole_path_hash`).
pub struct PathHasher(u64);

impl PathHasher {
    /// A hasher with nothing fed in yet.
    pub fn new() -> Self {
        Self(FNV_OFFSET_BASIS)
    }

    /// Fold the next chunk of the path's bytes in (FNV-1a).
    pub fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= *byte as u64;
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }

    /// The finished key. splitmix64's finalizer: avalanches every input bit across all
    /// 64 output bits. Not optional — raw FNV-1a barely mixes its LOW bits, which is
    /// exactly where hashbrown takes its bucket index.
    pub fn finish(self) -> u64 {
        let mut hash = self.0;
        hash ^= hash >> 30;
        hash = hash.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        hash ^= hash >> 27;
        hash = hash.wrapping_mul(0x94d0_49bb_1331_11eb);
        hash ^ (hash >> 31)
    }
}

impl Default for PathHasher {
    fn default() -> Self {
        Self::new()
    }
}

/// The `BuildHasher` for a map whose keys are ALREADY well-mixed 64-bit hashes: it
/// passes the key straight through instead of hashing it a second time.
///
/// Sound only because every key comes from [`hash_path`], which finalizes its output —
/// feeding a raw or weakly-mixed `u64` through this would cluster hashbrown's buckets.
// DEFAULT-OK: a stateless builder; there is no value in it to be wrong about.
#[derive(Debug, Default, Clone, Copy)]
pub struct PrehashedState;

impl BuildHasher for PrehashedState {
    type Hasher = PrehashedHasher;

    fn build_hasher(&self) -> PrehashedHasher {
        PrehashedHasher(FNV_OFFSET_BASIS)
    }
}

/// See [`PrehashedState`].
#[derive(Debug)]
pub struct PrehashedHasher(u64);

impl Hasher for PrehashedHasher {
    fn write_u64(&mut self, value: u64) {
        self.0 = value;
    }

    /// Never reached in practice (the key is a `u64`, which hashes via `write_u64`),
    /// but a `Hasher` has to handle any input, so fall back to FNV-1a rather than
    /// silently collapsing every byte string to one hash.
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= *byte as u64;
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hash is fixed, not `RandomState`: the same path hashes to the same value every
    /// time, in this run and in any other. Distinct paths (including near-identical ones)
    /// hash apart. A pinned value documents that the function is a stated constant rather
    /// than an implementation detail that can drift under the map.
    #[test]
    fn hash_path_is_deterministic_and_separates_similar_paths() {
        assert_eq!(
            hash_path("/Users/me/projects/cmdr"),
            hash_path("/Users/me/projects/cmdr")
        );
        assert_eq!(
            hash_path("/Users/me/projects/cmdr"),
            0xfc85_ae89_3f25_ed17,
            "the hash is a fixed function; update this only when changing it deliberately"
        );

        let similar = [
            "/Users/me/projects/cmdr",
            "/Users/me/projects/cmds",
            "/Users/me/projects/cmdr/",
            "/Users/me/projects/Cmdr",
            "/users/me/projects/cmdr",
            "",
            "/",
        ];
        let distinct: std::collections::HashSet<u64> = similar.iter().copied().map(hash_path).collect();
        assert_eq!(distinct.len(), similar.len(), "similar paths must not share a hash");
    }

    /// The finalizer earns its place: paths sharing a long prefix AND differing only late
    /// must land in different low bits, because that's where hashbrown takes its bucket
    /// index. Raw FNV-1a barely moves its low bits, so a bucket-index collision rate here
    /// is what catches dropping the mix.
    #[test]
    fn hash_path_spreads_sibling_paths_across_buckets() {
        const SIBLINGS: usize = 4096;
        let low_bits: std::collections::HashSet<u64> = (0..SIBLINGS)
            .map(|i| hash_path(&format!("/Users/me/Library/Application Support/vendor/cache/entry-{i}")) & 0xfff)
            .collect();
        // 4,096 hashes into 4,096 buckets fills ~63 % of them when the bits are random;
        // a hash that barely mixes its low bits collapses far below this.
        assert!(
            low_bits.len() > SIBLINGS / 2,
            "sibling paths filled only {} of {SIBLINGS} low-bit buckets",
            low_bits.len()
        );
    }

    /// Streaming the path in pieces is the same hash as hashing it whole, wherever the
    /// pieces split.
    #[test]
    fn a_streamed_path_hashes_like_the_whole_one() {
        let mut hasher = PathHasher::new();
        for piece in ["/Users", "/me/pro", "jects/cmdr"] {
            hasher.write(piece.as_bytes());
        }
        assert_eq!(hasher.finish(), hash_path("/Users/me/projects/cmdr"));
    }
}
