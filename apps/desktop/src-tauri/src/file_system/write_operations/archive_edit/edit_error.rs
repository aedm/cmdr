//! [`EditError`], the one failure type every archive-edit stage speaks, local
//! planning and the remote pull / upload / swap alike.
//!
//! A leaf on purpose: `engine` dispatches INTO `remote`, and both name this type,
//! so it lives below the two of them. Defining it in either one welds them into
//! a module cycle.

use super::super::types::WriteOperationError;

/// An archive-edit failure that separates a user cancel (archive untouched,
/// nothing to report as an error) from a genuine fault.
///
/// `pub(crate)` (not `pub(super)`) so the live-SMB and MTP integration suites can
/// drive `remote::pull_apply_upload_swap` directly against a real remote volume.
pub(crate) enum EditError {
    /// The user cancelled: a Stop prompt's oneshot sender was dropped, or the op
    /// was cancelled before a remote swap committed (the remote original is
    /// untouched).
    Cancelled,
    /// A real fault: an unreadable source, an unparseable archive, Stop under a
    /// pre-resolved policy, or a remote stage (pull, upload, or swap) failing.
    Op(WriteOperationError),
}
