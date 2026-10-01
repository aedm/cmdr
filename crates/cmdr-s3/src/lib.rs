#![warn(unused_crate_dependencies)]
#![deny(missing_docs)]
#![allow(
    dead_code,
    reason = "the protocol layer lands before the volume that calls it; drop this once `volume/` exists"
)]

//! Everything Cmdr says to an S3-compatible object store.

pub(crate) mod encoding;
pub(crate) mod error;
pub(crate) mod metadata;
pub(crate) mod multipart;
pub(crate) mod ops;
pub(crate) mod profile;
pub(crate) mod request;
pub(crate) mod sigv4;
pub(crate) mod xml;
