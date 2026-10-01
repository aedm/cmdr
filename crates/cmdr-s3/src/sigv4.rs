//! AWS Signature Version 4, the S3 flavor.
//!
//! Two modes. Header auth (`Authorization`) for every API call; query auth
//! (`X-Amz-Signature` in the URL) only for share links, because a signature in
//! a URL ends up in every log line that prints the URL. The S3 differences from
//! generic SigV4: the canonical URI is encoded once, and the payload hash
//! travels in `x-amz-content-sha256`, where `UNSIGNED-PAYLOAD` is allowed.

use std::fmt;
use std::time::{Duration, SystemTime};

use hmac::{Hmac, KeyInit, Mac};
use http::{HeaderName, HeaderValue};
use sha2::{Digest, Sha256};
use url::Url;

use crate::encoding::{canonical_query, encode_component};
use crate::request::{S3Request, SignedRequest};

const ALGORITHM: &str = "AWS4-HMAC-SHA256";
const SERVICE: &str = "s3";
const TERMINATOR: &str = "aws4_request";

/// The longest a presigned URL may live: seven days, S3's SigV4 ceiling.
pub(crate) const MAX_PRESIGN_EXPIRY: Duration = Duration::from_secs(604_800);

/// An access key pair. `Debug` never prints the secret.
#[derive(Clone)]
pub(crate) struct Credentials {
    pub access_key_id: String,
    secret_access_key: String,
}

impl Credentials {
    pub(crate) fn new(access_key_id: impl Into<String>, secret_access_key: impl Into<String>) -> Self {
        Self {
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
        }
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"<redacted>")
            .finish()
    }
}

/// A signing instant, as `YYYYMMDDTHHMMSSZ` in UTC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AmzTime {
    stamp: String,
}

impl AmzTime {
    pub(crate) fn new(at: SystemTime) -> Self {
        let utc = time::OffsetDateTime::from(at);
        Self {
            stamp: format!(
                "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
                utc.year(),
                u8::from(utc.month()),
                utc.day(),
                utc.hour(),
                utc.minute(),
                utc.second()
            ),
        }
    }

    /// `YYYYMMDDTHHMMSSZ`.
    pub(crate) fn stamp(&self) -> &str {
        &self.stamp
    }

    /// `YYYYMMDD`, the scope's date.
    fn date(&self) -> &str {
        &self.stamp[..8]
    }
}

/// What `x-amz-content-sha256` says about the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PayloadHash {
    /// `UNSIGNED-PAYLOAD`: for streamed bodies, so the bytes are read once.
    Unsigned,
    /// The body's SHA-256, lowercase hex.
    Sha256(String),
}

impl PayloadHash {
    pub(crate) fn of(bytes: &[u8]) -> Self {
        Self::Sha256(hex(&Sha256::digest(bytes)))
    }

    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Unsigned => "UNSIGNED-PAYLOAD",
            Self::Sha256(hash) => hash,
        }
    }
}

/// Who signs, where, and when.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Scope<'a> {
    pub credentials: &'a Credentials,
    /// The signing region: `us-east-1`, `auto` for R2, a location for Hetzner.
    pub region: &'a str,
    pub time: &'a AmzTime,
}

impl Scope<'_> {
    /// `<date>/<region>/s3/aws4_request`.
    fn credential_scope(&self) -> String {
        format!("{}/{}/{SERVICE}/{TERMINATOR}", self.time.date(), self.region)
    }
}

/// Signs `request` with header auth: adds `host`, `x-amz-date`,
/// `x-amz-content-sha256`, and `Authorization`, signing every header present.
pub(crate) fn sign(mut request: S3Request, scope: &Scope<'_>) -> SignedRequest {
    let payload = request.payload_hash();
    request.headers.insert(name("x-amz-date"), value(scope.time.stamp()));
    request
        .headers
        .insert(name("x-amz-content-sha256"), value(payload.as_str()));

    let headers = headers_to_sign(&request);
    let query = canonical_query(&request.query);
    let (canonical, signed_headers) = canonical_request(&request, &query, &headers, payload.as_str());
    let signature = signature(&string_to_sign(&canonical, scope), scope);
    let authorization = format!(
        "{ALGORITHM} Credential={}/{},SignedHeaders={signed_headers},Signature={signature}",
        scope.credentials.access_key_id,
        scope.credential_scope()
    );
    request.headers.insert(name("authorization"), value(&authorization));
    // Sent explicitly so the transport can't derive a different spelling
    // (an IPv6 literal, an explicit default port) from the URL than the one
    // signed above.
    request.headers.insert(name("host"), value(&request.host));

    let url = request.url();
    SignedRequest {
        method: request.method,
        url,
        headers: request.headers,
        body: request.body,
    }
}

/// Why a share link wasn't minted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PresignError {
    /// Zero, or longer than [`MAX_PRESIGN_EXPIRY`].
    ExpiryOutOfRange,
}

/// A presigned URL for `request` (a GET with no body), valid for `expires`.
/// Only `host` is signed, so anyone holding the URL can use it from anywhere.
pub(crate) fn presign(request: &S3Request, scope: &Scope<'_>, expires: Duration) -> Result<Url, PresignError> {
    if expires.is_zero() || expires > MAX_PRESIGN_EXPIRY {
        return Err(PresignError::ExpiryOutOfRange);
    }
    let mut query = request.query.clone();
    query.extend([
        ("X-Amz-Algorithm".to_string(), ALGORITHM.to_string()),
        (
            "X-Amz-Credential".to_string(),
            format!("{}/{}", scope.credentials.access_key_id, scope.credential_scope()),
        ),
        ("X-Amz-Date".to_string(), scope.time.stamp().to_string()),
        ("X-Amz-Expires".to_string(), expires.as_secs().to_string()),
        ("X-Amz-SignedHeaders".to_string(), "host".to_string()),
    ]);
    let canonical_query = canonical_query(&query);
    let host = [("host".to_string(), request.host.clone())];
    let (canonical, _) = canonical_request(request, &canonical_query, &host, PayloadHash::Unsigned.as_str());
    let signature = signature(&string_to_sign(&canonical, scope), scope);

    // The canonical query is already sorted and fully encoded, so it goes on
    // the wire as-is with the signature last, the order AWS's examples use.
    let text = format!(
        "{}://{}{}?{canonical_query}&X-Amz-Signature={}",
        request.scheme,
        request.host,
        request.path,
        encode_component(&signature)
    );
    Ok(Url::parse(&text).expect("a presigned URL is assembled from parsed, encoded parts"))
}

/// The canonical request and its `SignedHeaders` list, for headers already
/// lowercased, trimmed, and sorted by name.
fn canonical_request(
    request: &S3Request,
    query: &str,
    headers: &[(String, String)],
    payload: &str,
) -> (String, String) {
    let mut canonical_headers = String::new();
    for (name, value) in headers {
        canonical_headers.push_str(name);
        canonical_headers.push(':');
        canonical_headers.push_str(value);
        canonical_headers.push('\n');
    }
    let signed_headers: Vec<&str> = headers.iter().map(|(name, _)| name.as_str()).collect();
    let signed_headers = signed_headers.join(";");
    let path = if request.path.is_empty() { "/" } else { &request.path };
    let text = format!(
        "{}\n{path}\n{query}\n{canonical_headers}\n{signed_headers}\n{payload}",
        request.method.as_str()
    );
    (text, signed_headers)
}

fn string_to_sign(canonical_request: &str, scope: &Scope<'_>) -> String {
    format!(
        "{ALGORITHM}\n{}\n{}\n{}",
        scope.time.stamp(),
        scope.credential_scope(),
        hex(&Sha256::digest(canonical_request.as_bytes()))
    )
}

fn signature(string_to_sign: &str, scope: &Scope<'_>) -> String {
    let secret = format!("AWS4{}", scope.credentials.secret_access_key);
    let date_key = hmac(secret.as_bytes(), scope.time.date().as_bytes());
    let region_key = hmac(&date_key, scope.region.as_bytes());
    let service_key = hmac(&region_key, SERVICE.as_bytes());
    let signing_key = hmac(&service_key, TERMINATOR.as_bytes());
    hex(&hmac(&signing_key, string_to_sign.as_bytes()))
}

/// The headers to sign: every one present plus `host`, lowercased, values
/// trimmed with inner runs of spaces collapsed, sorted by name.
fn headers_to_sign(request: &S3Request) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = request
        .headers
        .iter()
        .filter(|(name, _)| name.as_str() != "host")
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                trim_all(&String::from_utf8_lossy(value.as_bytes())),
            )
        })
        .collect();
    headers.push(("host".to_string(), request.host.clone()));
    headers.sort();
    headers
}

/// SigV4's `Trimall`: strip both ends and collapse each run of spaces to one.
fn trim_all(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts a key of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// A header name this module adds. All are valid lowercase tokens.
fn name(text: &'static str) -> HeaderName {
    HeaderName::from_static(text)
}

/// A header value this module builds: hex, a timestamp, or the
/// `Authorization` line, all ASCII without control characters.
fn value(text: &str) -> HeaderValue {
    HeaderValue::from_str(text).expect("a signing header value is printable ASCII")
}

#[cfg(test)]
#[path = "sigv4_test.rs"]
mod sigv4_test;
