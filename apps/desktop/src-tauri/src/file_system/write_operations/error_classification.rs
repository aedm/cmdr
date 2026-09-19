//! Maps raw `std::io::Error` values into typed `WriteOperationError` variants.
//!
//! Only `errno` and `ErrorKind` are consulted, never the formatted message. The
//! `IoResultExt` extension trait and the
//! `From<std::io::Error>` impl are the two entry points local-FS code uses to
//! attach a typed variant (and a path) to an IO failure; the native copy calls
//! (`macos_copy`, `linux_copy`) use `classify_copy_io_error`, which also picks
//! the path.

use std::path::Path;

use super::types::{ReadOnlySide, WriteOperationError};

/// Classifies a raw `std::io::Error` into a specific `WriteOperationError` variant.
///
/// Only `errno` and `ErrorKind` are consulted — never the formatted message.
/// Backend errors (SMB, MTP, etc.) are typed and flow through
/// `transfer/volume/transfer_error.rs::map_volume_error`, so this function only sees
/// `std::io::Error` values produced by local-FS calls, which always carry a
/// `raw_os_error()` on Unix. Pre-fix the function had a lowercase-substring
/// fallback (`"disconnect"`, `"read-only"`, `"connection"`, `"operation not
/// permitted"`, …) that quietly misclassified errors on localized macOS
/// (the wording localizes; the substrings don't match) and was the exact
/// shape AGENTS.md bans.
pub(super) fn classify_io_error(e: &std::io::Error, path: String) -> WriteOperationError {
    #[cfg(unix)]
    if let Some(code) = e.raw_os_error() {
        match code {
            libc::EROFS => {
                // EROFS came back from a WRITE, so the refusing half is the
                // destination by construction.
                return WriteOperationError::ReadOnlyDevice {
                    path,
                    device_name: None,
                    side: ReadOnlySide::Destination,
                };
            }
            libc::ENAMETOOLONG => return WriteOperationError::NameTooLong { path },
            // The refused write measured nothing, so it's the sizeless variant.
            libc::ENOSPC | libc::EDQUOT => return WriteOperationError::DestinationFull { path },
            libc::ENOTCONN | libc::ENETDOWN | libc::ENETUNREACH | libc::EHOSTUNREACH | libc::ETIMEDOUT => {
                return WriteOperationError::ConnectionInterrupted { path };
            }
            // Both are a drive that isn't there any more: `ENODEV` from a mount
            // that's gone, `ENXIO` from a device node whose hardware left. The
            // SIDE is filled in at the operation's boundary, where the volumes
            // it was handed at start are still known (`transfer_sides.rs`).
            libc::ENODEV | libc::ENXIO => return WriteOperationError::DeviceDisconnected { path, side: None },
            _ => {} // Fall through to ErrorKind classification
        }
    }

    match e.kind() {
        std::io::ErrorKind::NotFound => WriteOperationError::SourceNotFound { path },
        // The errno rides along (Rust folds `EACCES` and `EPERM` into one kind, and
        // they're opposite advice), and the probe asks the OS which folder actually
        // said no rather than letting the copy guess from the operation's shape.
        std::io::ErrorKind::PermissionDenied => {
            let refused_folder = super::validation::refusing_folder(Path::new(&path));
            WriteOperationError::permission_denied(path, e.to_string(), e.raw_os_error(), refused_folder)
        }
        std::io::ErrorKind::AlreadyExists => WriteOperationError::DestinationExists { path },
        _ => WriteOperationError::IoError {
            path,
            message: e.to_string(),
        },
    }
}

/// Classifies a failure from a native copy call (`copyfile`, `copy_file_range`),
/// which touches both ends, so the errno also decides which path the error names.
///
/// A refusal of the WRITE names the destination (read-only, full, a name it can't
/// hold, no permission, occupied); a missing source, a dropped link, or anything
/// unclassified names the source the user copied from. ❌ Don't give a platform
/// its own copy of this table: the macOS one drifted and sent `EROFS` to the user
/// as a generic failure.
#[cfg(unix)]
pub(super) fn classify_copy_io_error(err: &std::io::Error, source: &Path, destination: &Path) -> WriteOperationError {
    let refused_by_destination = matches!(
        err.kind(),
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::AlreadyExists
    ) || matches!(
        err.raw_os_error(),
        Some(libc::EROFS | libc::ENOSPC | libc::EDQUOT | libc::ENAMETOOLONG)
    );
    let path = if refused_by_destination { destination } else { source };
    classify_io_error(err, path.display().to_string())
}

/// Extension trait for converting `io::Result` to `Result<T, WriteOperationError>` with path
/// context.
pub(super) trait IoResultExt<T> {
    fn with_path(self, path: &Path) -> Result<T, WriteOperationError>;
}

impl<T> IoResultExt<T> for std::io::Result<T> {
    fn with_path(self, path: &Path) -> Result<T, WriteOperationError> {
        self.map_err(|e| classify_io_error(&e, path.display().to_string()))
    }
}

impl From<std::io::Error> for WriteOperationError {
    fn from(err: std::io::Error) -> Self {
        classify_io_error(&err, String::new())
    }
}

impl WriteOperationError {
    /// Whether this outcome is expected, recoverable control flow rather than a
    /// genuine failure.
    ///
    /// Callers log expected-recoverable outcomes at `warn`, not `error`, so they
    /// stay below the error-reporter's auto-report threshold (error level IS that
    /// threshold — see [`crate::error_reporter`]'s `CLAUDE.md`). Without this an
    /// encrypted-archive extract would queue a false-positive auto error report on
    /// every password prompt and every wrong attempt.
    ///
    /// `ArchiveNeedsPassword` is the sole case today: extracting from an encrypted
    /// archive raises it purely to prompt the user for a password and retry.
    pub(crate) fn is_expected_recoverable(&self) -> bool {
        matches!(self, WriteOperationError::ArchiveNeedsPassword { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full disk and a spent quota have one fix, room, and nothing was measured:
    /// neither may read as a generic failure, nor as "needs 0 bytes".
    #[cfg(unix)]
    #[test]
    fn a_full_disk_and_a_spent_quota_are_a_full_destination() {
        for errno in [libc::ENOSPC, libc::EDQUOT] {
            let err = classify_io_error(
                &std::io::Error::from_raw_os_error(errno),
                "/Volumes/Stick/a".to_string(),
            );
            assert!(
                matches!(&err, WriteOperationError::DestinationFull { path } if path == "/Volumes/Stick/a"),
                "errno {errno}: got {err:?}"
            );
        }
    }

    #[test]
    fn archive_needs_password_is_expected_recoverable() {
        // Both the fresh prompt and the wrong-attempt re-prompt are recoverable.
        for wrong_attempt in [false, true] {
            let err = WriteOperationError::ArchiveNeedsPassword {
                path: "/secret.zip".to_string(),
                wrong_attempt,
            };
            assert!(
                err.is_expected_recoverable(),
                "needs-password must stay below the auto-report threshold (wrong_attempt={wrong_attempt})"
            );
        }
    }

    #[test]
    fn genuine_failures_are_not_expected_recoverable() {
        let failures = [
            WriteOperationError::IoError {
                path: "/x".to_string(),
                message: "boom".to_string(),
            },
            WriteOperationError::SourceNotFound { path: "/x".to_string() },
            WriteOperationError::permission_denied("/x".to_string(), "denied".to_string(), None, None),
        ];
        for err in failures {
            assert!(
                !err.is_expected_recoverable(),
                "genuine failure must still log at error and auto-report: {err:?}"
            );
        }
    }
}
