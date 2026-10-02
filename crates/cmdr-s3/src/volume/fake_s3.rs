//! A tiny in-process S3 for cells that need a server to misbehave in one
//! precise way the Docker fixtures can't:
//!
//! - it commits a PUT whose whole body arrived and then waits `answer_after`
//!   before saying so (R2's slow answer);
//! - [`FakeS3::hang_up_after_commit`]: it commits a PUT or a
//!   `CompleteMultipartUpload` and then drops the connection unanswered (a
//!   link that dies right after the server published);
//! - [`FakeS3::keep_cut_off_bodies`]: it stores what arrived of a PUT cut off
//!   mid-body, the way VersityGW does, where S3 publishes nothing;
//! - it refuses a listing prefix past S3's 1,024-byte key ceiling with `400
//!   InvalidRequest` the way B2 does, where other servers answer an empty page.
//!
//! It speaks path style over plain HTTP, one request per connection, and
//! knows HEAD, PUT, DELETE, `ListObjectsV2`, and the multipart calls. A cell
//! reaches it through a bucket place on R2 ([`FakeS3::volume`]): R2 refuses a
//! short body, so an overwrite goes as one PUT.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cmdr_fs::ignore_poison::IgnorePoison as _;
use cmdr_fs::volume::host::VolumeHost;
use percent_encoding::percent_decode_str;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::S3Volume;
use crate::params::{S3ConnectionParams, S3Provider};
use crate::sigv4::Credentials;
use crate::transport::S3Client;

const ACCOUNT: &str = "acct";
pub(super) const BUCKET: &str = "bucket";
const KEY_ID: &str = "AKIATEST";

/// One stored object: its length, ETag, and the `x-amz-meta-*` lines it was
/// written with.
#[derive(Clone)]
pub(super) struct Stored {
    pub len: usize,
    pub etag: String,
    pub meta: Vec<String>,
}

/// A multipart upload in progress: its key, the metadata its creation named,
/// and each part's length.
struct Upload {
    key: String,
    meta: Vec<String>,
    parts: BTreeMap<u32, usize>,
}

#[derive(Default)]
struct World {
    /// By decoded key.
    objects: BTreeMap<String, Stored>,
    uploads: BTreeMap<String, Upload>,
    writes: usize,
    /// Every listing prefix asked for, decoded.
    listed: Vec<String>,
    hang_up_after_commit: bool,
    keep_cut_off_bodies: bool,
    /// The decoded key of every object request, in order.
    keys_seen: Vec<String>,
}

pub(super) struct FakeS3 {
    addr: SocketAddr,
    world: Arc<Mutex<World>>,
}

impl FakeS3 {
    pub(super) async fn start(answer_after: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let world = Arc::new(Mutex::new(World::default()));
        let serving = Arc::clone(&world);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let world = Arc::clone(&serving);
                tokio::spawn(async move {
                    let Some((head, body_len, length)) = read_request(&mut socket).await else {
                        return;
                    };
                    let Some(response) = answer(&world, &head, body_len, length, answer_after).await else {
                        return;
                    };
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self { addr, world }
    }

    /// From now on, a PUT or a completion is committed and then left
    /// unanswered: the connection drops after the server published.
    pub(super) fn hang_up_after_commit(&self) {
        self.world.lock_ignore_poison().hang_up_after_commit = true;
    }

    /// From now on, a PUT cut off mid-body stores what arrived.
    pub(super) fn keep_cut_off_bodies(&self) {
        self.world.lock_ignore_poison().keep_cut_off_bodies = true;
    }

    pub(super) fn object(&self, key: &str) -> Option<Stored> {
        self.world.lock_ignore_poison().objects.get(key).cloned()
    }

    pub(super) fn seed(&self, key: &str, len: usize) {
        self.world.lock_ignore_poison().objects.insert(
            key.to_string(),
            Stored {
                len,
                etag: "\"original\"".into(),
                meta: Vec::new(),
            },
        );
    }

    /// Every listing prefix the volume asked for.
    pub(super) fn listed(&self) -> Vec<String> {
        self.world.lock_ignore_poison().listed.clone()
    }

    /// How many multipart uploads are still open.
    pub(super) fn open_uploads(&self) -> usize {
        self.world.lock_ignore_poison().uploads.len()
    }

    /// How many requests named `key` (any method, the multipart calls too).
    pub(super) fn requests_about(&self, key: &str) -> usize {
        self.world
            .lock_ignore_poison()
            .keys_seen
            .iter()
            .filter(|seen| *seen == key)
            .count()
    }

    /// A bucket place on R2, its endpoint dialing this fake over plain HTTP.
    pub(super) fn volume(&self) -> S3Volume {
        self.volume_for(S3Provider::R2 {
            account_id: ACCOUNT.into(),
        })
    }

    /// A bucket place on `provider`'s preset, its endpoint dialing this fake
    /// over plain HTTP. Path-style presets only (everyone but AWS).
    pub(super) fn volume_for(&self, provider: S3Provider) -> S3Volume {
        let params = S3ConnectionParams::new(provider, KEY_ID, Some(BUCKET)).expect("valid params");
        let mut profile = params.profile().expect("a profile");
        profile.scheme = "http".into();
        let host = profile.endpoint_host.clone();
        let client = S3Client::resolving(
            profile,
            Credentials::new(KEY_ID, "secret".to_string()),
            &[host],
            self.addr,
        );
        S3Volume::assemble("fake", "s3-fake", params, client, VolumeHost::detached())
    }
}

const NOT_FOUND: &str = "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

fn ok_xml(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/xml\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn error(status: &str, code: &str) -> String {
    let body = format!("<Error><Code>{code}</Code><Message>m</Message></Error>");
    format!(
        "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// One query parameter's decoded value; a valueless one answers `""`.
fn param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (key == name).then(|| percent_decode_str(value).decode_utf8_lossy().into_owned())
    })
}

fn meta_lines(head: &str) -> Vec<String> {
    head.lines()
        .filter(|l| l.to_ascii_lowercase().starts_with("x-amz-meta-"))
        .map(str::to_string)
        .collect()
}

/// What the fake says to one request; `None` hangs up without an answer.
async fn answer(
    world: &Mutex<World>,
    head: &str,
    body_len: usize,
    length: usize,
    answer_after: Duration,
) -> Option<String> {
    let mut line = head.lines().next().unwrap_or_default().split(' ');
    let method = line.next().unwrap_or_default().to_string();
    let target = line.next().unwrap_or_default().to_string();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let key = path
        .strip_prefix(&format!("/{BUCKET}/"))
        .map(|key| percent_decode_str(key).decode_utf8_lossy().into_owned());
    if let Some(key) = &key {
        world.lock_ignore_poison().keys_seen.push(key.clone());
    }
    let upload_id = param(query, "uploadId");
    let response = match (method.as_str(), key, upload_id) {
        ("PUT", Some(_), Some(id)) if body_len == length => {
            let mut world = world.lock_ignore_poison();
            let number: u32 = param(query, "partNumber").and_then(|n| n.parse().ok()).unwrap_or(0);
            match world.uploads.get_mut(&id) {
                Some(upload) => {
                    upload.parts.insert(number, body_len);
                    format!("HTTP/1.1 200 OK\r\netag: \"p{number}\"\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                }
                None => error("404 Not Found", "NoSuchUpload"),
            }
        }
        ("PUT", Some(_), Some(_)) => return None,
        ("PUT", Some(key), None) if body_len < length => {
            let mut world = world.lock_ignore_poison();
            if world.keep_cut_off_bodies {
                world.writes += 1;
                let etag = format!("\"v{}\"", world.writes);
                world.objects.insert(
                    key,
                    Stored {
                        len: body_len,
                        etag,
                        meta: meta_lines(head),
                    },
                );
            }
            // Cut off: a well-behaved S3 publishes nothing.
            return None;
        }
        ("PUT", Some(key), None) => {
            let (etag, hang) = {
                let mut world = world.lock_ignore_poison();
                world.writes += 1;
                let etag = format!("\"v{}\"", world.writes);
                world.objects.insert(
                    key,
                    Stored {
                        len: body_len,
                        etag: etag.clone(),
                        meta: meta_lines(head),
                    },
                );
                (etag, world.hang_up_after_commit)
            };
            if hang {
                return None;
            }
            tokio::time::sleep(answer_after).await;
            format!("HTTP/1.1 200 OK\r\netag: {etag}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
        }
        ("POST", Some(key), None) if param(query, "uploads").is_some() => {
            let mut world = world.lock_ignore_poison();
            world.writes += 1;
            let id = format!("up{}", world.writes);
            world.uploads.insert(
                id.clone(),
                Upload {
                    key: key.clone(),
                    meta: meta_lines(head),
                    parts: BTreeMap::new(),
                },
            );
            ok_xml(&format!(
                "<InitiateMultipartUploadResult><Bucket>{BUCKET}</Bucket><Key>{key}</Key><UploadId>{id}</UploadId></InitiateMultipartUploadResult>"
            ))
        }
        ("POST", Some(key), Some(id)) => {
            let mut world = world.lock_ignore_poison();
            let Some(upload) = world.uploads.remove(&id) else {
                return Some(error("404 Not Found", "NoSuchUpload"));
            };
            let etag = format!("\"m{}-{}\"", world.writes, upload.parts.len());
            world.objects.insert(
                key.clone(),
                Stored {
                    len: upload.parts.values().sum(),
                    etag: etag.clone(),
                    meta: upload.meta,
                },
            );
            if world.hang_up_after_commit {
                return None;
            }
            ok_xml(&format!(
                "<CompleteMultipartUploadResult><Bucket>{BUCKET}</Bucket><Key>{key}</Key><ETag>{etag}</ETag></CompleteMultipartUploadResult>"
            ))
        }
        ("DELETE", Some(_), Some(id)) => match world.lock_ignore_poison().uploads.remove(&id) {
            Some(_) => "HTTP/1.1 204 No Content\r\nconnection: close\r\n\r\n".into(),
            None => error("404 Not Found", "NoSuchUpload"),
        },
        ("HEAD", Some(key), None) => match world.lock_ignore_poison().objects.get(&key) {
            Some(stored) => format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\netag: {}\r\nlast-modified: Fri, 02 Oct 2026 10:00:00 GMT\r\n{}connection: close\r\n\r\n",
                stored.len,
                stored.etag,
                stored.meta.iter().map(|m| format!("{m}\r\n")).collect::<String>()
            ),
            None => NOT_FOUND.into(),
        },
        ("DELETE", Some(key), None) => {
            world.lock_ignore_poison().objects.remove(&key);
            "HTTP/1.1 204 No Content\r\nconnection: close\r\n\r\n".into()
        }
        ("GET", None, None) if param(query, "uploads").is_some() => {
            let world = world.lock_ignore_poison();
            let uploads: String = world
                .uploads
                .iter()
                .map(|(id, upload)| format!("<Upload><Key>{}</Key><UploadId>{id}</UploadId></Upload>", upload.key))
                .collect();
            ok_xml(&format!(
                "<ListMultipartUploadsResult><Bucket>{BUCKET}</Bucket><IsTruncated>false</IsTruncated>{uploads}</ListMultipartUploadsResult>"
            ))
        }
        ("GET", None, None) if query.contains("list-type=2") => list(world, query),
        _ => "HTTP/1.1 501 Not Implemented\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".into(),
    };
    Some(response)
}

/// `ListObjectsV2`, every match as `Contents` (no delimiter folding), and ❗
/// B2's refusal of a prefix no key could carry.
fn list(world: &Mutex<World>, query: &str) -> String {
    let prefix = param(query, "prefix").unwrap_or_default();
    let mut world = world.lock_ignore_poison();
    world.listed.push(prefix.clone());
    if prefix.len() > 1024 {
        return error("400 Bad Request", "InvalidRequest");
    }
    let contents: String = world
        .objects
        .iter()
        .filter(|(key, _)| key.starts_with(&prefix))
        .map(|(key, stored)| {
            format!(
                "<Contents><Key>{key}</Key><Size>{}</Size><ETag>{}</ETag><LastModified>2026-10-02T10:00:00.000Z</LastModified></Contents>",
                stored.len, stored.etag
            )
        })
        .collect();
    ok_xml(&format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult><Name>{BUCKET}</Name><Prefix>{prefix}</Prefix><IsTruncated>false</IsTruncated>{contents}</ListBucketResult>"
    ))
}

/// The request head, the body bytes that arrived, and the `Content-Length`.
async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<(String, usize, usize)> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 65536];
    let head_end = loop {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let length: usize = head
        .lines()
        .skip(1)
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .unwrap_or(0);
    let mut body_len = buffer.len() - head_end;
    while body_len < length {
        match socket.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => body_len += read,
        }
    }
    Some((head, body_len, length))
}
