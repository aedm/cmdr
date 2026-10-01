//! Fixtures for the Docker-backed S3 suites, on both sides of the crate
//! boundary. Gated behind the `testing` feature, so it exists in dev targets
//! and in no shipped build. The stack itself:
//! `apps/desktop/test/s3-servers/start.sh`.
//!
//! ❗ The stack is machine-wide and its objects persist across runs (named
//! volumes), so every cell works under a key prefix of its own
//! ([`scratch_prefix`]) and never assumes an empty bucket. Seeding goes through
//! this crate's own request builders and transport rather than the volume, so a
//! cell about the volume's write path never seeds through the code it tests.

use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use cmdr_fs::volume::{StreamLength, Volume, VolumeError, VolumeReadStream};

use cmdr_fs::volume::host::VolumeHost;
use cmdr_fs::volume::host::credentials::InMemoryCredentials;
use cmdr_fs::volume::host::events::{RecordingVolumeEvents, VolumeEventSink};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{S3Volume, connect_s3_volume};
use crate::ops::{self, ObjectMetadata, Overwrite};
use crate::params::{S3ConnectionParams, S3Provider};
use crate::request::Body;
use crate::sigv4::Credentials;
use crate::transport::{QUERY_BUDGET, S3Client};

/// The access key id both fixture servers know. Public on purpose: these are
/// fixtures (`apps/desktop/test/s3-servers/README.md`).
pub const FIXTURE_ACCESS_KEY: &str = "GK00000000000000000000c0de";

/// Its secret: `c0de` sixteen times, Garage's 64-hex format.
pub fn fixture_secret() -> String {
    "c0de".repeat(16)
}

/// The bucket every cell works in, under a prefix of its own.
pub const FIXTURE_BUCKET: &str = "cmdr-test";

/// The second bucket both servers start with.
pub const FIXTURE_BUCKET_2: &str = "cmdr-test-2";

/// One fixture server: the key its port env var carries and the port its
/// compose file publishes by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixtureService {
    /// `VERSITYGW` or `GARAGE`.
    pub key: &'static str,
    /// The compose default.
    pub port: u16,
}

/// VersityGW: behaves like AWS on conditional writes. The default target.
pub const VERSITYGW: FixtureService = FixtureService {
    key: "VERSITYGW",
    port: 14480,
};

/// Garage: ignores every write precondition, and answers a wrong secret with
/// `AccessDenied` rather than `SignatureDoesNotMatch`.
pub const GARAGE: FixtureService = FixtureService {
    key: "GARAGE",
    port: 14481,
};

/// Both servers, for a cell that runs against each.
pub const FIXTURE_SERVICES: [FixtureService; 2] = [VERSITYGW, GARAGE];

/// The host port a fixture service publishes: `S3_FIXTURE_{key}_PORT`, else
/// the compose default.
pub fn fixture_port(service: FixtureService) -> u16 {
    std::env::var(format!("S3_FIXTURE_{}_PORT", service.key))
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(service.port)
}

/// The fixture server as an "Other S3-compatible" provider: plain HTTP on
/// loopback, path style, `us-east-1`.
pub fn fixture_provider(service: FixtureService) -> S3Provider {
    S3Provider::Other {
        endpoint: Url::parse(&format!("http://127.0.0.1:{}", fixture_port(service)))
            .expect("a fixture URL is well-formed by construction"),
        region: None,
        path_style: true,
    }
}

/// The params for one place on a fixture server: a bucket, or the account root.
pub fn fixture_params(service: FixtureService, bucket: Option<&str>) -> S3ConnectionParams {
    S3ConnectionParams::new(fixture_provider(service), FIXTURE_ACCESS_KEY, bucket)
        .expect("the fixture provider is valid by construction")
}

/// A host with the fixture secret stored for both servers, and a recording
/// event sink. Detached otherwise.
pub fn fixture_host() -> VolumeHost {
    fixture_host_with_secret(&fixture_secret())
}

/// A host storing `secret` for both servers, for a cell that needs the WRONG
/// one stored.
pub fn fixture_host_with_secret(secret: &str) -> VolumeHost {
    let mut credentials = InMemoryCredentials::new();
    for service in FIXTURE_SERVICES {
        credentials = credentials.with_entry(
            &fixture_params(service, None).credential_service(),
            Some(FIXTURE_ACCESS_KEY),
            FIXTURE_ACCESS_KEY,
            secret,
        );
    }
    VolumeHost::builder()
        .credentials(Arc::new(credentials))
        .events(Arc::new(RecordingVolumeEvents::new()) as Arc<dyn VolumeEventSink>)
        .build()
}

/// A [`fixture_host`] whose backends keep their durable state under `dir`
/// (`VolumeHost::state_dir`), for a cell about S3's unfinished-upload record.
pub fn fixture_host_with_state(dir: &Path) -> VolumeHost {
    let mut credentials = InMemoryCredentials::new();
    for service in FIXTURE_SERVICES {
        credentials = credentials.with_entry(
            &fixture_params(service, None).credential_service(),
            Some(FIXTURE_ACCESS_KEY),
            FIXTURE_ACCESS_KEY,
            &fixture_secret(),
        );
    }
    VolumeHost::builder()
        .credentials(Arc::new(credentials))
        .events(Arc::new(RecordingVolumeEvents::new()) as Arc<dyn VolumeEventSink>)
        .state_root(dir)
        .build()
}

/// Connects to one place on a fixture server, panicking with a pointer at the
/// stack script if it isn't up.
pub async fn connect_fixture(service: FixtureService, bucket: Option<&str>) -> S3Volume {
    connect_fixture_with_host(service, bucket, fixture_host()).await
}

/// [`connect_fixture`] through `host`.
pub async fn connect_fixture_with_host(service: FixtureService, bucket: Option<&str>, host: VolumeHost) -> S3Volume {
    let params = fixture_params(service, bucket);
    let volume_id = cmdr_fs::volume::s3_volume_id(params.host(), params.port(), FIXTURE_ACCESS_KEY, bucket);
    match connect_s3_volume("fixture", &volume_id, params, host, CancellationToken::new()).await {
        Ok(volume) => volume,
        Err(e) => panic!(
            "the S3 fixture {} refused a connection ({e:?}); is the stack up? apps/desktop/test/s3-servers/start.sh",
            service.key
        ),
    }
}

/// A token unique to this process, minted once. ❗ Random rather than the pid:
/// a suite inside a container sees small pids another run sees too.
fn run_token() -> &'static str {
    static TOKEN: OnceLock<String> = OnceLock::new();
    TOKEN.get_or_init(|| uuid::Uuid::new_v4().simple().to_string()[..12].to_string())
}

/// A key prefix no other cell or run uses: `cmdr-test-<run>-<n>-<label>/`. The
/// objects stay behind on the fixture (its volume is wiped with `down -v`);
/// nothing here relies on a clean bucket.
pub fn scratch_prefix(label: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "cmdr-test-{}-{}-{label}/",
        run_token(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// A signed client for a fixture server, for seeding.
fn seeding_client(service: FixtureService) -> S3Client {
    let params = fixture_params(service, None);
    let profile = params.profile().expect("the fixture provider is valid by construction");
    S3Client::new(profile, Credentials::new(FIXTURE_ACCESS_KEY, fixture_secret()))
        .expect("a client builds for the fixture")
}

/// What to put at one key.
pub struct Seed<'a> {
    /// The full key, prefix included.
    pub key: &'a str,
    /// The object's bytes. Empty for a folder marker (`a/`).
    pub bytes: &'a [u8],
    /// The `x-amz-meta-mtime` to write, rclone's format.
    pub mtime: Option<SystemTime>,
}

/// Puts objects in `bucket` on a fixture server, up to 32 at a time,
/// panicking on the first that doesn't land.
pub async fn seed(service: FixtureService, bucket: &str, seeds: &[Seed<'_>]) {
    let client = Arc::new(seeding_client(service));
    for batch in seeds.chunks(32) {
        let mut puts = Vec::with_capacity(batch.len());
        for seed in batch {
            let metadata = ObjectMetadata {
                mtime: seed.mtime,
                write_token: None,
                carried: Vec::new(),
            };
            let built = ops::put_object(
                client.profile(),
                bucket,
                seed.key,
                seed.bytes.len() as u64,
                Overwrite::Replace,
                &metadata,
            )
            .unwrap_or_else(|e| panic!("building a PUT for {:?}: {e:?}", seed.key));
            let mut request = built.request;
            request.body = Body::Bytes(seed.bytes.to_vec());
            let client = Arc::clone(&client);
            let key = seed.key.to_string();
            puts.push(tokio::spawn(async move {
                let answer = client
                    .exchange(request, QUERY_BUDGET)
                    .await
                    .unwrap_or_else(|e| panic!("seeding {key:?}: {e}"));
                assert!(
                    answer.status.is_success(),
                    "seeding {key:?} answered {}: {}",
                    answer.status,
                    answer.text()
                );
            }));
        }
        for put in puts {
            put.await.expect("a seeding task finished");
        }
    }
}

/// Puts `bytes` at `key` carrying `token` as its `x-amz-meta-cmdr-write`, the
/// way an overwrite's temp a crash left behind looks.
pub async fn seed_with_token(service: FixtureService, bucket: &str, key: &str, bytes: &[u8], token: &str) {
    let client = seeding_client(service);
    let metadata = ObjectMetadata {
        mtime: None,
        write_token: Some(token.to_string()),
        carried: Vec::new(),
    };
    let built = ops::put_object(
        client.profile(),
        bucket,
        key,
        bytes.len() as u64,
        Overwrite::Replace,
        &metadata,
    )
    .unwrap_or_else(|e| panic!("building a PUT for {key:?}: {e:?}"));
    let mut request = built.request;
    request.body = Body::Bytes(bytes.to_vec());
    let answer = client
        .exchange(request, QUERY_BUDGET)
        .await
        .unwrap_or_else(|e| panic!("seeding {key:?}: {e}"));
    assert!(answer.status.is_success(), "seeding {key:?} answered {}", answer.status);
}

/// Puts `bytes()` at `key` unless an object of that exact length is already
/// there, for a big object that would otherwise pile up on the fixture's disk
/// once per run. ❗ The key is fixed and shared across runs, so treat it as
/// read-only.
pub async fn seed_once(service: FixtureService, bucket: &str, key: &str, len: usize, bytes: impl FnOnce() -> Vec<u8>) {
    let client = seeding_client(service);
    let head = ops::head_object(client.profile(), bucket, key).expect("a fixture key builds");
    let answer = client
        .exchange(head, QUERY_BUDGET)
        .await
        .unwrap_or_else(|e| panic!("probing {key:?}: {e}"));
    if answer.status.is_success() && answer.header("content-length") == Some(len.to_string().as_str()) {
        return;
    }
    let bytes = bytes();
    assert_eq!(
        bytes.len(),
        len,
        "seed_once: the generator must make exactly the length asked for ({len} B)"
    );
    seed(service, bucket, &[object(key, &bytes)]).await;
}

/// `len` bytes that say where they are: line `n` reads `<tag> <n>` padded to
/// a fixed width, so a misplaced window shows which bytes it got.
pub fn self_describing_bytes(len: usize, tag: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut line = 0u64;
    while out.len() < len {
        out.extend_from_slice(format!("{tag} {line:015}\n").as_bytes());
        line += 1;
    }
    out.truncate(len);
    out
}

/// The plain seed: `bytes` at `key`, no metadata.
pub fn object<'a>(key: &'a str, bytes: &'a [u8]) -> Seed<'a> {
    Seed {
        key,
        bytes,
        mtime: None,
    }
}

/// A time a cell writes as an mtime and expects back: whole seconds, well in
/// the past, so it can't be mistaken for an upload time.
pub fn distant_mtime() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_354_040_105)
}

/// Starts a multipart upload straight through the protocol, with no Cmdr
/// record of it: what another tool's live upload looks like to a sweep.
pub async fn start_foreign_upload(service: FixtureService, bucket: &str, key: &str) -> String {
    let client = seeding_client(service);
    let request = ops::create_multipart_upload(client.profile(), bucket, key, &ObjectMetadata::default())
        .expect("a fixture key builds");
    let answer = client
        .exchange(request, QUERY_BUDGET)
        .await
        .unwrap_or_else(|e| panic!("starting an upload of {key:?}: {e}"));
    assert!(
        answer.status.is_success(),
        "starting an upload answered {}",
        answer.status
    );
    crate::xml::parse_initiate_multipart(&answer.text())
        .expect("an InitiateMultipartUploadResult")
        .upload_id
}

/// Every unfinished upload under `prefix`, as `(key, upload id)`.
pub async fn unfinished_uploads(service: FixtureService, bucket: &str, prefix: &str) -> Vec<(String, String)> {
    let client = seeding_client(service);
    let mut found = Vec::new();
    let mut markers: Option<(String, String)> = None;
    loop {
        let request = ops::list_multipart_uploads(
            client.profile(),
            bucket,
            prefix,
            markers.as_ref().map(|(key, id)| (key.as_str(), id.as_str())),
        )
        .expect("a fixture listing builds");
        let answer = client
            .exchange(request, QUERY_BUDGET)
            .await
            .unwrap_or_else(|e| panic!("listing uploads under {prefix:?}: {e}"));
        assert!(answer.status.is_success(), "listing uploads answered {}", answer.status);
        let page = crate::xml::parse_list_multipart_uploads(&answer.text()).expect("a ListMultipartUploadsResult");
        found.extend(page.uploads.into_iter().map(|upload| (upload.key, upload.upload_id)));
        match (page.is_truncated, page.next_key_marker, page.next_upload_id_marker) {
            (true, Some(key), Some(id)) => markers = Some((key, id)),
            _ => break,
        }
    }
    found
}

/// Aborts one upload straight through the protocol, for a cell's cleanup.
pub async fn abort_foreign_upload(service: FixtureService, bucket: &str, key: &str, upload_id: &str) {
    let client = seeding_client(service);
    let request = ops::abort_multipart_upload(client.profile(), bucket, key, upload_id).expect("a fixture key builds");
    let _ = client.exchange(request, QUERY_BUDGET).await;
}

/// The `x-amz-meta-mtime` an object carries, as stored.
pub async fn stored_mtime_header(service: FixtureService, bucket: &str, key: &str) -> Option<String> {
    let client = seeding_client(service);
    let request = ops::head_object(client.profile(), bucket, key).expect("a fixture key builds");
    let answer = client
        .exchange(request, QUERY_BUDGET)
        .await
        .unwrap_or_else(|e| panic!("probing {key:?}: {e}"));
    answer.header(crate::metadata::MTIME_HEADER).map(str::to_string)
}

/// Bytes as a copy's source: in pieces of `piece` bytes, with a known length
/// or not, and the modification time a destination should keep.
pub struct BytesSource {
    bytes: Vec<u8>,
    offset: usize,
    piece: usize,
    known: bool,
    mtime: Option<SystemTime>,
}

impl BytesSource {
    /// `bytes` of known length in 1 MiB pieces, no mtime.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            offset: 0,
            piece: 1024 * 1024,
            known: true,
            mtime: None,
        }
    }

    /// The same bytes with no length declared up front.
    pub fn of_unknown_length(mut self) -> Self {
        self.known = false;
        self
    }

    /// The source file's modification time.
    pub fn modified_at(mut self, mtime: SystemTime) -> Self {
        self.mtime = Some(mtime);
        self
    }
}

impl VolumeReadStream for BytesSource {
    fn next_chunk(&mut self) -> Pin<Box<dyn Future<Output = Option<Result<Vec<u8>, VolumeError>>> + Send + '_>> {
        Box::pin(async move {
            if self.offset >= self.bytes.len() {
                return None;
            }
            let end = (self.offset + self.piece).min(self.bytes.len());
            let piece = self.bytes[self.offset..end].to_vec();
            self.offset = end;
            Some(Ok(piece))
        })
    }

    fn total_size(&self) -> StreamLength {
        if self.known {
            StreamLength::Known(self.bytes.len() as u64)
        } else {
            StreamLength::Unknown
        }
    }

    fn bytes_read(&self) -> u64 {
        self.offset as u64
    }

    fn modified_at(&self) -> Option<SystemTime> {
        self.mtime
    }
}

/// Everything at `path`, read back through the volume.
pub async fn read_back(volume: &S3Volume, path: &Path) -> Vec<u8> {
    let mut stream = volume
        .open_read_stream(path)
        .await
        .unwrap_or_else(|e| panic!("reading {} back: {e:?}", path.display()));
    let mut bytes = Vec::new();
    while let Some(piece) = stream.next_chunk().await {
        bytes.extend(piece.unwrap_or_else(|e| panic!("reading {} back: {e:?}", path.display())));
    }
    bytes
}
