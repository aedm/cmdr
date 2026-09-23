//! The entry a folder holds under another Unicode spelling of a name.
//!
//! A byte-exact destination (an SMB share, SFTP, a phone) keeps `café` composed
//! and `café` decomposed as two entries and finds each only by its own bytes. A
//! lookup in the other spelling misses an entry a person calls the same name, and
//! a write that then believes the name is free stands a second, identical-looking
//! entry beside the user's. So every write that asks "is this name free?" asks
//! here once its own lookup has missed, and a look-alike is taken, never free.
//!
//! Only a difference in FORM counts (`cmdr_fs::name_fold::differ_only_in_form`):
//! `Report` and `report` read as two names, and a case-sensitive destination
//! keeps both on purpose. `DETAILS.md` § "Look-alike names".

use std::path::Path;

use cmdr_fs::name_fold::differ_only_in_form;

use crate::file_system::listing::FileEntry;
use crate::file_system::volume::{Volume, VolumeError};

/// What a folder holds under another spelling of one name.
#[derive(Debug)]
pub(crate) enum LookAlike {
    /// Nothing: the name is free as far as spelling goes.
    None,
    /// One entry, which IS the name as far as anyone can see. A write to the
    /// name lands on this entry's own path.
    One(Box<FileEntry>),
    /// Two or more, none spelled the way the caller asked. Picking one would be
    /// a guess, and a guess that overwrites is data loss.
    Several,
}

/// `name`'s look-alike among `entries`, a folder's listing.
///
/// An entry spelled exactly `name` means the lookup that sent the caller here
/// raced a create, so the name answers for itself: [`LookAlike::None`].
pub(crate) fn among<'a>(entries: impl IntoIterator<Item = &'a FileEntry>, name: &str) -> LookAlike {
    let mut found: Option<&FileEntry> = None;
    let mut several = false;
    for entry in entries {
        if entry.name == name {
            return LookAlike::None;
        }
        if differ_only_in_form(&entry.name, name) {
            several |= found.is_some();
            found = Some(entry);
        }
    }
    match (found, several) {
        (_, true) => LookAlike::Several,
        (Some(entry), false) => LookAlike::One(Box::new(entry.clone())),
        (None, false) => LookAlike::None,
    }
}

/// `name`'s look-alike in `dir` on `volume`, for a caller whose exact lookup of
/// `dir/name` just answered `NotFound`.
///
/// Free when it can't matter: an ASCII name has one spelling, and a volume that
/// finds names in any form ([`Volume::matches_names_in_any_unicode_form`]) already
/// answered with that miss. Otherwise one listing of `dir`. A `dir` that isn't
/// there holds nothing; any other listing failure is the caller's to fail the
/// item on, ❌ never "nothing is there".
pub(crate) async fn look_alike_in(volume: &dyn Volume, dir: &Path, name: &str) -> Result<LookAlike, VolumeError> {
    if name.is_ascii() || volume.matches_names_in_any_unicode_form() {
        return Ok(LookAlike::None);
    }
    match volume.list_directory(dir, None).await {
        Ok(entries) => Ok(among(&entries, name)),
        Err(VolumeError::NotFound(_)) => Ok(LookAlike::None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
#[path = "look_alike_tests.rs"]
mod tests;
