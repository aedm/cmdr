//! Each builder's method, URL, headers, and body.

use std::time::{Duration, UNIX_EPOCH};

use http::Method;
use url::Url;

use super::*;
use crate::profile::{Preset, ProviderProfile};
use crate::request::{Body, S3Request};
use crate::xml::build::CompletedPart;

const MTIME: u64 = 1_354_040_105;

fn aws() -> ProviderProfile {
    ProviderProfile::from_preset(&Preset::Aws {
        region: "us-east-1".into(),
    })
    .unwrap()
}

fn r2() -> ProviderProfile {
    ProviderProfile::from_preset(&Preset::R2 {
        account_id: "acct".into(),
    })
    .unwrap()
}

fn b2() -> ProviderProfile {
    ProviderProfile::from_preset(&Preset::B2 {
        region: "us-east-005".into(),
    })
    .unwrap()
}

fn hetzner() -> ProviderProfile {
    ProviderProfile::from_preset(&Preset::Hetzner {
        location: "fsn1".into(),
    })
    .unwrap()
}

fn header<'a>(request: &'a S3Request, name: &str) -> Option<&'a str> {
    request.headers.get(name).map(|v| v.to_str().unwrap())
}

fn with_mtime() -> ObjectMetadata {
    ObjectMetadata {
        mtime: Some(UNIX_EPOCH + Duration::from_secs(MTIME)),
    }
}

#[test]
fn list_buckets_hits_the_account_root() {
    let request = list_buckets(&aws(), None);
    assert_eq!(request.method, Method::GET);
    assert_eq!(request.url().as_str(), "https://s3.us-east-1.amazonaws.com/");
    assert_eq!(
        list_buckets(&aws(), Some("tok/en")).url().as_str(),
        "https://s3.us-east-1.amazonaws.com/?continuation-token=tok%2Fen"
    );
}

#[test]
fn list_objects_asks_for_v2_url_encoded_keys_one_folder_deep() {
    let params = ListObjectsParams {
        prefix: "photos/2024 trip/",
        delimiter: Some("/"),
        continuation_token: Some("abc+="),
        max_keys: Some(500),
    };
    let request = list_objects(&aws(), "photos", &params).unwrap();
    assert_eq!(
        request.url().as_str(),
        "https://photos.s3.us-east-1.amazonaws.com/?continuation-token=abc%2B%3D&delimiter=%2F\
         &encoding-type=url&list-type=2&max-keys=500&prefix=photos%2F2024%20trip%2F"
    );

    // Path style, and the bucket root without a prefix.
    let root = list_objects(&r2(), "b", &ListObjectsParams::default()).unwrap();
    assert_eq!(
        root.url().as_str(),
        "https://acct.r2.cloudflarestorage.com/b?encoding-type=url&list-type=2"
    );
}

#[test]
fn get_object_carries_its_range() {
    let open_ended = get_object(&aws(), "photos", "a.jpg", Some(ByteRange { start: 100, end: None })).unwrap();
    assert_eq!(header(&open_ended, "range"), Some("bytes=100-"));
    let bounded = get_object(&aws(), "photos", "a.jpg", Some(ByteRange { start: 0, end: Some(9) })).unwrap();
    assert_eq!(header(&bounded, "range"), Some("bytes=0-9"));
    assert_eq!(
        header(&get_object(&aws(), "photos", "a.jpg", None).unwrap(), "range"),
        None
    );
}

#[test]
fn put_object_streams_its_body_and_writes_the_rclone_mtime() {
    let built = put_object(&aws(), "photos", "a.jpg", 1234, Overwrite::Replace, &with_mtime()).unwrap();
    assert!(!built.check_first);
    assert_eq!(built.request.method, Method::PUT);
    assert_eq!(built.request.body, Body::Streamed { length: 1234 });
    assert_eq!(header(&built.request, "x-amz-meta-mtime"), Some("1354040105"));
    assert_eq!(header(&built.request, "if-none-match"), None);
}

#[test]
fn a_refused_overwrite_uses_the_header_where_the_provider_has_one_and_checks_first_where_not() {
    let on_aws = put_object(&aws(), "b", "k", 1, Overwrite::Refuse, &ObjectMetadata::default()).unwrap();
    assert_eq!(header(&on_aws.request, "if-none-match"), Some("*"));
    assert!(!on_aws.check_first);

    let on_b2 = put_object(&b2(), "b", "k", 1, Overwrite::Refuse, &ObjectMetadata::default()).unwrap();
    assert_eq!(header(&on_b2.request, "if-none-match"), None);
    assert!(on_b2.check_first);

    let parts = [CompletedPart {
        number: 1,
        etag: "\"e\"".into(),
    }];
    let complete_on_r2 = complete_multipart_upload(&r2(), "b", "k", "up", &parts, Overwrite::Refuse).unwrap();
    assert!(complete_on_r2.check_first, "R2 has no conditional complete");
}

#[test]
fn a_downgraded_profile_stops_sending_the_header() {
    let aws = aws();
    aws.downgrade(ConditionalOp::Put);
    let built = put_object(&aws, "b", "k", 1, Overwrite::Refuse, &ObjectMetadata::default()).unwrap();
    assert_eq!(header(&built.request, "if-none-match"), None);
    assert!(built.check_first);
}

#[test]
fn multipart_upload_runs_create_part_complete_and_abort() {
    let profile = aws();

    let create = create_multipart_upload(&profile, "photos", "big.iso", &with_mtime()).unwrap();
    assert_eq!(create.method, Method::POST);
    assert_eq!(
        create.url().as_str(),
        "https://photos.s3.us-east-1.amazonaws.com/big.iso?uploads"
    );
    assert_eq!(header(&create, "x-amz-meta-mtime"), Some("1354040105"));

    let part = upload_part(&profile, "photos", "big.iso", "u/1", 3, 64).unwrap();
    assert_eq!(part.method, Method::PUT);
    assert_eq!(
        part.url().as_str(),
        "https://photos.s3.us-east-1.amazonaws.com/big.iso?partNumber=3&uploadId=u%2F1"
    );
    assert_eq!(part.body, Body::Streamed { length: 64 });

    let parts = [
        CompletedPart {
            number: 1,
            etag: "\"a\"".into(),
        },
        CompletedPart {
            number: 2,
            etag: "\"b\"".into(),
        },
    ];
    let complete = complete_multipart_upload(&profile, "photos", "big.iso", "u/1", &parts, Overwrite::Refuse).unwrap();
    assert_eq!(complete.request.method, Method::POST);
    assert_eq!(
        complete.request.url().as_str(),
        "https://photos.s3.us-east-1.amazonaws.com/big.iso?uploadId=u%2F1"
    );
    assert_eq!(header(&complete.request, "if-none-match"), Some("*"));
    let Body::Bytes(body) = &complete.request.body else {
        panic!("complete carries an XML body")
    };
    assert!(String::from_utf8_lossy(body).contains("<PartNumber>2</PartNumber>"));

    let abort = abort_multipart_upload(&profile, "photos", "big.iso", "u/1").unwrap();
    assert_eq!(abort.method, Method::DELETE);
    assert_eq!(
        abort.url().as_str(),
        "https://photos.s3.us-east-1.amazonaws.com/big.iso?uploadId=u%2F1"
    );
}

#[test]
fn upload_part_copy_names_its_source_and_byte_range() {
    let source = CopySource {
        bucket: "photos",
        key: "2024/a b.mov",
    };
    let request = upload_part_copy(&aws(), "archive", "a.mov", "up", 2, source, (64, 127)).unwrap();
    assert_eq!(header(&request, "x-amz-copy-source"), Some("/photos/2024/a%20b.mov"));
    assert_eq!(header(&request, "x-amz-copy-source-range"), Some("bytes=64-127"));
    assert_eq!(request.body, Body::Empty);
}

#[test]
fn copy_object_keeps_metadata_by_default_and_replaces_it_on_request() {
    let source = CopySource {
        bucket: "b",
        key: "old name.txt",
    };
    let keep = copy_object(
        &aws(),
        source,
        "b",
        "new.txt",
        Overwrite::Replace,
        &MetadataDirective::Copy,
    )
    .unwrap();
    assert_eq!(header(&keep.request, "x-amz-copy-source"), Some("/b/old%20name.txt"));
    assert_eq!(header(&keep.request, "x-amz-metadata-directive"), None);

    let replace = copy_object(
        &aws(),
        source,
        "b",
        "new.txt",
        Overwrite::Replace,
        &MetadataDirective::Replace(with_mtime()),
    )
    .unwrap();
    assert_eq!(header(&replace.request, "x-amz-metadata-directive"), Some("REPLACE"));
    assert_eq!(header(&replace.request, "x-amz-meta-mtime"), Some("1354040105"));
}

#[test]
fn a_refused_copy_overwrite_uses_r2s_own_header() {
    let source = CopySource { bucket: "b", key: "a" };
    let on_r2 = copy_object(&r2(), source, "b", "c", Overwrite::Refuse, &MetadataDirective::Copy).unwrap();
    assert_eq!(header(&on_r2.request, "cf-copy-destination-if-none-match"), Some("*"));
    assert_eq!(header(&on_r2.request, "if-none-match"), None);
    let on_aws = copy_object(&aws(), source, "b", "c", Overwrite::Refuse, &MetadataDirective::Copy).unwrap();
    assert_eq!(header(&on_aws.request, "if-none-match"), Some("*"));
}

#[test]
fn hetzner_refuses_a_cross_bucket_copy_before_sending_it() {
    let source = CopySource {
        bucket: "one",
        key: "a",
    };
    assert_eq!(
        copy_object(
            &hetzner(),
            source,
            "two",
            "a",
            Overwrite::Replace,
            &MetadataDirective::Copy
        )
        .err(),
        Some(BuildError::CrossBucketCopy)
    );
    assert!(
        copy_object(
            &hetzner(),
            source,
            "one",
            "b",
            Overwrite::Replace,
            &MetadataDirective::Copy
        )
        .is_ok()
    );
    assert_eq!(
        upload_part_copy(&hetzner(), "two", "a", "up", 1, source, (0, 1)).err(),
        Some(BuildError::CrossBucketCopy)
    );
}

#[test]
fn delete_objects_sends_a_quiet_batch_with_its_md5() {
    let request = delete_objects(&aws(), "photos", &["a.txt"]).unwrap();
    assert_eq!(request.method, Method::POST);
    assert_eq!(
        request.url().as_str(),
        "https://photos.s3.us-east-1.amazonaws.com/?delete"
    );
    // `openssl md5 -binary | base64` over the same body.
    assert_eq!(header(&request, "content-md5"), Some("urczhL0jfmvY7/F/VGSOXw=="));
    assert_eq!(header(&request, "content-type"), Some("application/xml"));
}

#[test]
fn delete_objects_takes_one_to_a_thousand_keys() {
    assert_eq!(
        delete_objects(&aws(), "b", &[]).err(),
        Some(BuildError::DeleteBatchSize)
    );
    let many: Vec<String> = (0..=MAX_DELETE_KEYS).map(|i| format!("k{i}")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    assert_eq!(
        delete_objects(&aws(), "b", &refs).err(),
        Some(BuildError::DeleteBatchSize)
    );
    assert!(delete_objects(&aws(), "b", &refs[..MAX_DELETE_KEYS]).is_ok());
}

#[test]
fn list_multipart_uploads_pages_by_markers() {
    let first = list_multipart_uploads(&r2(), "b", "", None).unwrap();
    assert_eq!(
        first.url().as_str(),
        "https://acct.r2.cloudflarestorage.com/b?encoding-type=url&uploads"
    );

    let next = list_multipart_uploads(&r2(), "b", "dir/", Some(("dir/k", "u1"))).unwrap();
    assert_eq!(
        next.url().as_str(),
        "https://acct.r2.cloudflarestorage.com/b?encoding-type=url&key-marker=dir%2Fk&prefix=dir%2F\
         &upload-id-marker=u1&uploads"
    );
}

#[test]
fn a_share_link_is_a_presigned_get_for_the_object() {
    let credentials = Credentials::new("AKIAIOSFODNN7EXAMPLE", "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY");
    let now = UNIX_EPOCH + Duration::from_secs(1_369_353_600);

    let link = share_link(
        &r2(),
        &credentials,
        "photos",
        "a b.jpg",
        now,
        Duration::from_secs(3_600),
    )
    .unwrap();

    let url = Url::parse(link.as_str()).unwrap();
    assert_eq!(url.host_str(), Some("acct.r2.cloudflarestorage.com"));
    assert_eq!(url.path(), "/photos/a%20b.jpg");
    let query = url.query().unwrap();
    assert!(query.contains("X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fauto%2Fs3%2Faws4_request"));
    assert!(query.contains("X-Amz-Expires=3600"));
    assert!(query.contains("X-Amz-Signature="));
    assert_eq!(
        share_link(&r2(), &credentials, "photos", "a", now, Duration::from_secs(604_801)),
        Err(ShareLinkError::ExpiryOutOfRange)
    );
}
