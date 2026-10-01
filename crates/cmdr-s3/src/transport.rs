//! The HTTP client: signs each request, sends it, and reads the answer.
//!
//! ❗ **`reqwest` is confined to this file.** Everything else works in
//! [`S3Request`]s going out and [`Answer`]s coming back, and hands a transport
//! failure straight back here to be judged ([`map_transport_error`],
//! [`classify_connect_error`]), so a client swap is one file's problem.

use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use cmdr_fs::volume::VolumeError;
use cmdr_fs::volume::liveness::Liveness;
use futures_util::Stream;
use http::{HeaderMap, StatusCode};
use log::debug;

use crate::ops;
use crate::profile::ProviderProfile;
use crate::refusal::{BucketCheck, BucketList, S3ConnectError, judge_head_bucket, judge_list_buckets};
use crate::request::{Body, S3Request};
use crate::sigv4::{AmzTime, Credentials, Scope, sign};

/// The connect timeout on every request, and the connect probe's total budget
/// per request.
///
/// ❌ Never `ClientBuilder::read_timeout`: its sleep runs from the request
/// going out until the response HEADERS arrive, so it's a total budget on an
/// upload's whole body phase (`crates/cmdr-webdav/src/transport.rs` has the
/// evidence). The read and write paths (M4, M5) carry none.
pub(crate) const REQUEST_BUDGET: Duration = Duration::from_secs(10);

/// One listing page's or one HEAD's total budget: bounded work, though a
/// page on a slow or throttled server may take a while.
pub(crate) const QUERY_BUDGET: Duration = Duration::from_secs(60);

/// The bytes of a streamed request body ([`S3Client::upload`]): every piece the
/// transport sends, in order. An `Err` aborts the request on the wire, which is
/// how a refused source or a cancel keeps S3 from publishing anything.
pub(crate) type UploadBody = Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>;

/// A `CompleteMultipartUpload`'s total budget. Longer than a query's: AWS may
/// take minutes to assemble a big object, sending whitespace meanwhile, which
/// the body read counts as heard.
pub(crate) const COMPLETE_BUDGET: Duration = Duration::from_secs(15 * 60);

/// What a request came back with. The body is read whole: every answer this
/// carries is a small XML document or nothing (a HEAD).
#[derive(Debug)]
pub(crate) struct Answer {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Answer {
    /// The body as text, lossily: XML from a well-behaved server, an HTML page
    /// or nothing from anything else.
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// A header's value, when it's there and printable.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
}

/// An answer whose body is still on the wire ([`S3Client::open`]).
pub(crate) struct Opened {
    pub status: StatusCode,
    pub headers: HeaderMap,
    response: reqwest::Response,
    /// The client's silence watch: every chunk is the server being there.
    liveness: Arc<Liveness>,
    volume_id: String,
    path: String,
}

impl Opened {
    /// A header's value, when it's there and printable.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    /// The next piece of the body as it arrives, `None` at its end. A
    /// transport failure comes back in the `Volume` vocabulary. Copied out
    /// once, because `VolumeReadStream` hands out owned bytes anyway.
    pub(crate) async fn chunk(&mut self) -> Option<Result<Vec<u8>, VolumeError>> {
        match self.response.chunk().await {
            Ok(Some(chunk)) => {
                self.liveness.heard();
                Some(Ok(chunk.to_vec()))
            }
            Ok(None) => None,
            Err(e) => Some(Err(map_transport_error(&e, &self.volume_id, &self.path))),
        }
    }

    /// The rest of the body as text, for an error answer (a small XML
    /// document). Whatever doesn't arrive within `budget` is left out, so a
    /// misbehaving server can't hold a failed read open.
    pub(crate) async fn text(mut self, budget: Duration) -> String {
        let mut body = Vec::new();
        let _ = tokio::time::timeout(budget, async {
            while let Some(Ok(chunk)) = self.chunk().await {
                body.extend_from_slice(&chunk);
            }
        })
        .await;
        String::from_utf8_lossy(&body).into_owned()
    }
}

/// One signed client for one account on one endpoint.
pub(crate) struct S3Client {
    http: reqwest::Client,
    /// The silence watch's line to the server: the same settings with pooling
    /// OFF, so every probe dials fresh. ❗ A pooled probe could ride the very
    /// connection that went quiet.
    fresh: reqwest::Client,
    profile: ProviderProfile,
    credentials: Credentials,
    /// What the server has said lately (`cmdr_fs::volume::liveness`). Dies
    /// with this client: a reconnect builds a new one.
    liveness: Arc<Liveness>,
}

impl S3Client {
    /// Builds the client. Redirects are off: S3 answers a request for a bucket
    /// in another region with a 301, and following it would re-send a request
    /// signed for the wrong host.
    pub(crate) fn new(profile: ProviderProfile, credentials: Credentials) -> Result<Self, S3ConnectError> {
        let builder = || {
            reqwest::Client::builder()
                .user_agent("Cmdr")
                .connect_timeout(REQUEST_BUDGET)
                .redirect(reqwest::redirect::Policy::none())
        };
        let build_failed = |e: reqwest::Error| S3ConnectError::Transport(e.to_string());
        Ok(Self {
            http: builder().build().map_err(build_failed)?,
            // `0` turns hyper-util's pool off outright (verified on 0.1.20,
            // `pool::Config::is_enabled`, 2026-09-23).
            fresh: builder().pool_max_idle_per_host(0).build().map_err(build_failed)?,
            profile,
            credentials,
            liveness: Arc::new(Liveness::new()),
        })
    }

    /// The provider profile every request is built against.
    pub(crate) fn profile(&self) -> &ProviderProfile {
        &self.profile
    }

    /// This client's silence watch.
    pub(crate) fn liveness(&self) -> &Arc<Liveness> {
        &self.liveness
    }

    /// Signs `request` now, sends it, and reads the whole answer within
    /// `budget`, noting the headers and every body chunk as the server being
    /// there. ❗ Every request goes out through here, or its bytes never count
    /// against silence.
    ///
    /// ❗ For bodies held in memory only. A `Body::Streamed` request goes out
    /// with no body and the server refuses it (S3 answers a missing
    /// `Content-Length` with 411); the streaming write path brings its own
    /// sender.
    pub(crate) async fn exchange(&self, request: S3Request, budget: Duration) -> Result<Answer, reqwest::Error> {
        let time = AmzTime::new(SystemTime::now());
        let scope = Scope {
            credentials: &self.credentials,
            region: &self.profile.region,
            time: &time,
        };
        let signed = sign(request, &scope);
        let mut builder = self
            .http
            .request(signed.method, signed.url)
            .headers(signed.headers)
            .timeout(budget);
        if let Body::Bytes(bytes) = signed.body {
            builder = builder.body(bytes);
        }
        let mut response = builder.send().await?;
        self.liveness.heard();
        let status = response.status();
        let headers = response.headers().clone();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            self.liveness.heard();
            body.extend_from_slice(&chunk);
        }
        Ok(Answer { status, headers, body })
    }

    /// Signs `request` (a `Body::Streamed { length }`, so `UNSIGNED-PAYLOAD`)
    /// and sends `body` as its bytes with `Content-Length: length`, then reads
    /// the answer whole (a PUT answers nothing, an error a small XML document).
    ///
    /// ❗ `Content-Length` always, ❌ never a chunked body: S3 answers 411 to a
    /// PUT without one, and a body that ends short makes hyper abort the
    /// request, which S3 never publishes. ❌ No `.timeout()`: an upload has no
    /// total budget, and silence is the watch's to judge (the body source
    /// counts every piece it hands over as heard).
    pub(crate) async fn upload(&self, request: S3Request, body: UploadBody) -> Result<Answer, reqwest::Error> {
        let length = match request.body {
            Body::Streamed { length } => length,
            Body::Empty | Body::Bytes(_) => 0,
        };
        let time = AmzTime::new(SystemTime::now());
        let scope = Scope {
            credentials: &self.credentials,
            region: &self.profile.region,
            time: &time,
        };
        let signed = sign(request, &scope);
        let mut response = self
            .http
            .request(signed.method, signed.url)
            .headers(signed.headers)
            .header(reqwest::header::CONTENT_LENGTH, length)
            .body(reqwest::Body::wrap_stream(body))
            .send()
            .await?;
        self.liveness.heard();
        let status = response.status();
        let headers = response.headers().clone();
        let mut answer = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            self.liveness.heard();
            answer.extend_from_slice(&chunk);
        }
        Ok(Answer {
            status,
            headers,
            body: answer,
        })
    }

    /// Signs `request` now and sends it, handing back the answer with its body
    /// still on the wire, for a GET whose body may be any size.
    ///
    /// ❗ No `.timeout()` on the request: that would be a total budget on the
    /// whole body, and a multi-GB download has none. Only the wait for the
    /// headers is bounded (`QUERY_BUDGET`); the body's budget is per chunk, in
    /// the caller ([`Opened::chunk`] counts each one as heard).
    pub(crate) async fn open(&self, request: S3Request, volume_id: &str, path: &str) -> Result<Opened, VolumeError> {
        let time = AmzTime::new(SystemTime::now());
        let scope = Scope {
            credentials: &self.credentials,
            region: &self.profile.region,
            time: &time,
        };
        let signed = sign(request, &scope);
        let sent = self
            .http
            .request(signed.method, signed.url)
            .headers(signed.headers)
            .send();
        let response = match tokio::time::timeout(QUERY_BUDGET, sent).await {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => return Err(map_transport_error(&e, volume_id, path)),
            Err(_elapsed) => return Err(VolumeError::ConnectionTimeout(path.to_string())),
        };
        self.liveness.heard();
        Ok(Opened {
            status: response.status(),
            headers: response.headers().clone(),
            response,
            liveness: Arc::clone(&self.liveness),
            volume_id: volume_id.to_string(),
            path: path.to_string(),
        })
    }

    /// A presigned GET for `key`, valid for `expires` from now (`ops::share_link`).
    /// Here because the credentials are: computed offline, nothing is sent.
    /// ❗ The URL carries a signature that reads the object; ❌ never log it.
    pub(crate) fn share_link(
        &self,
        bucket: &str,
        key: &str,
        expires: Duration,
    ) -> Result<url::Url, ops::ShareLinkError> {
        ops::share_link(
            &self.profile,
            &self.credentials,
            bucket,
            key,
            SystemTime::now(),
            expires,
        )
    }

    /// Whether the server answers at all, on a fresh connection: an unsigned
    /// HEAD on the endpoint, any status counting. The watch applies the budget.
    pub(crate) async fn ping(&self) -> bool {
        let url = format!("{}://{}/", self.profile.scheme, self.profile.endpoint_host);
        self.fresh.head(url).send().await.is_ok()
    }

    /// The connect probe: `ListBuckets`, then `HeadBucket` when the place is a
    /// bucket, judged by `refusal.rs`'s table.
    ///
    /// ❗ `ListBuckets` goes first even for a bucket: it's the one call whose
    /// error BODY tells a wrong secret (`SignatureDoesNotMatch`) from a key
    /// without rights. A HEAD has no body to say which.
    pub(crate) async fn probe(&self, bucket: Option<&str>) -> Result<(), S3ConnectError> {
        debug!(target: "volume", "s3 probe: ListBuckets on {}", self.profile.endpoint_host);
        let listed = self
            .exchange(ops::list_buckets(&self.profile, None), REQUEST_BUDGET)
            .await
            .map_err(|e| classify_connect_error(&e))?;
        let bucket = match (
            judge_list_buckets(listed.status, &listed.text(), bucket.is_some()),
            bucket,
        ) {
            (BucketList::Refused(error), _) => return Err(error),
            (BucketList::Listed, None) => return Ok(()),
            (BucketList::FallBack, None) => return Err(S3ConnectError::BucketListRefused),
            (BucketList::Listed | BucketList::FallBack, Some(bucket)) => bucket,
        };
        // A name that can't be a bucket (a `/` in it) isn't one this endpoint has.
        let head = ops::head_bucket(&self.profile, bucket).map_err(|_| S3ConnectError::NoSuchBucket)?;
        let answer = self
            .exchange(head, REQUEST_BUDGET)
            .await
            .map_err(|e| classify_connect_error(&e))?;
        match judge_head_bucket(answer.status, answer.header("x-amz-bucket-region")) {
            BucketCheck::Open => Ok(()),
            BucketCheck::Refused(error) => Err(error),
        }
    }
}

/// Turns a `reqwest` failure (no status came back) into the `Volume`
/// vocabulary, by its typed predicates.
///
/// A connection that couldn't be made or was cut mid-flight is the volume
/// being gone, which is what starts the reconnect loop; a timeout is its own
/// variant, ❌ never read as a lost server (a slow page isn't one).
pub(crate) fn map_transport_error(err: &reqwest::Error, volume_id: &str, path: &str) -> VolumeError {
    if err.is_timeout() {
        return VolumeError::ConnectionTimeout(path.to_string());
    }
    if err.is_connect() || err.is_request() {
        debug!(
            "S3 path={path:?}: source=backend, backend=s3, error_kind=disconnected, detail={:?}",
            cmdr_fs::log_detail::LogDetail(&err.to_string())
        );
        return VolumeError::DeviceDisconnected(volume_id.to_string());
    }
    VolumeError::IoError {
        message: err.to_string(),
        raw_os_error: None,
    }
}

/// Classifies a `reqwest` failure on the CONNECT probe.
///
/// A TLS refusal reaches here as a connect error whose source chain carries an
/// `io::Error` of kind `InvalidData`, which is how `tokio-rustls` wraps every
/// handshake refusal. ❗ Judged by the typed `ErrorKind`, ❌ never the message.
pub(crate) fn classify_connect_error(err: &reqwest::Error) -> S3ConnectError {
    if err.is_timeout() {
        return S3ConnectError::TimedOut;
    }
    if err.is_connect() {
        if has_tls_refusal(err) {
            return S3ConnectError::CertificateUntrusted;
        }
        return S3ConnectError::Unreachable(err.to_string());
    }
    if err.is_request() {
        return S3ConnectError::Unreachable(err.to_string());
    }
    S3ConnectError::Transport(err.to_string())
}

/// Whether an `io::Error` of kind `InvalidData` sits anywhere under `err`.
fn has_tls_refusal(err: &reqwest::Error) -> bool {
    let mut source = std::error::Error::source(err);
    while let Some(inner) = source {
        if let Some(io) = inner.downcast_ref::<std::io::Error>()
            && io.kind() == std::io::ErrorKind::InvalidData
        {
            return true;
        }
        source = inner.source();
    }
    false
}
