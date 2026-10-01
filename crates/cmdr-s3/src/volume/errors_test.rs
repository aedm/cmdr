//! An S3 error in the `Volume` vocabulary, carrying the path it was about.

use http::StatusCode;

use super::map_s3_error;
use crate::error::S3Error;
use cmdr_fs::volume::VolumeError;

fn from_code(status: StatusCode, code: &str) -> S3Error {
    S3Error::from_response(status, &format!("<Error><Code>{code}</Code></Error>"))
}

#[test]
fn a_missing_key_or_bucket_is_not_found_and_names_the_path() {
    for error in [
        from_code(StatusCode::NOT_FOUND, "NoSuchKey"),
        from_code(StatusCode::NOT_FOUND, "NoSuchBucket"),
        S3Error::from_status(StatusCode::NOT_FOUND),
    ] {
        assert!(
            matches!(map_s3_error(&error, "/b/a.txt"), VolumeError::NotFound(path) if path == "/b/a.txt"),
            "{error}"
        );
    }
}

/// The only precondition Cmdr sends is a no-overwrite one (`If-None-Match: *`
/// or R2's copy header), so a 412 means the name is taken and what's there
/// stayed: the same refusal every other backend gives a `CreateNew`.
#[test]
fn a_failed_no_overwrite_precondition_is_already_exists_on_the_path() {
    for error in [
        from_code(StatusCode::PRECONDITION_FAILED, "PreconditionFailed"),
        S3Error::from_status(StatusCode::PRECONDITION_FAILED),
    ] {
        assert!(
            matches!(map_s3_error(&error, "/b/a.txt"), VolumeError::AlreadyExists(path) if path == "/b/a.txt"),
            "{error}"
        );
    }
}

#[test]
fn an_archived_object_is_cold_storage_on_the_path() {
    // Glacier Flexible Retrieval and Deep Archive answer a read with
    // `InvalidObjectState` (403) until the object is restored. It must stay
    // apart from a refusal: the keys are fine, the bytes just aren't there yet.
    let error = from_code(StatusCode::FORBIDDEN, "InvalidObjectState");
    assert!(
        matches!(map_s3_error(&error, "/b/old.tar"), VolumeError::ColdStorage(path) if path == "/b/old.tar"),
        "{error}"
    );
}

#[test]
fn a_refusal_is_permission_denied_on_the_path() {
    for error in [
        from_code(StatusCode::FORBIDDEN, "AccessDenied"),
        from_code(StatusCode::FORBIDDEN, "SignatureDoesNotMatch"),
        from_code(StatusCode::FORBIDDEN, "InvalidAccessKeyId"),
        S3Error::from_status(StatusCode::FORBIDDEN),
    ] {
        assert!(
            matches!(map_s3_error(&error, "/b"), VolumeError::PermissionDenied { path, .. } if path == "/b"),
            "{error}"
        );
    }
}

#[test]
fn an_operation_the_server_lacks_is_not_supported() {
    assert!(matches!(
        map_s3_error(&from_code(StatusCode::NOT_IMPLEMENTED, "NotImplemented"), "/b"),
        VolumeError::NotSupported
    ));
}

#[test]
fn anything_else_is_an_io_error_naming_the_code_and_status() {
    let VolumeError::IoError { message, .. } =
        map_s3_error(&from_code(StatusCode::SERVICE_UNAVAILABLE, "SlowDown"), "/b")
    else {
        panic!("a throttle is an I/O error");
    };
    assert!(message.contains("SlowDown") && message.contains("503"), "{message}");
}
