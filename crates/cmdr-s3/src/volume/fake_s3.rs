//! A tiny in-process S3 for cells that need a server to misbehave in one
//! precise way the Docker fixtures can't: it commits a PUT whose whole body
//! arrived and then waits `answer_after` before saying so (R2's slow answer),
//! and it refuses a listing prefix past S3's 1,024-byte key ceiling with `400
//! InvalidRequest` the way B2 does, where other servers answer an empty page.
//!
//! It speaks path style over plain HTTP, one request per connection, and
//! knows HEAD, PUT, DELETE, and `ListObjectsV2`. A cell reaches it through a
//! bucket place on R2 ([`FakeS3::volume`]): R2 refuses a short body, so an
//! overwrite goes as one PUT.

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
use crate::profile::{Preset, ProviderProfile};
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

#[derive(Default)]
struct World {
    /// By decoded key.
    objects: BTreeMap<String, Stored>,
    writes: usize,
    /// Every listing prefix asked for, decoded.
    listed: Vec<String>,
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

    /// A bucket place on R2, its endpoint dialing this fake over plain HTTP.
    pub(super) fn volume(&self) -> S3Volume {
        let provider = S3Provider::R2 {
            account_id: ACCOUNT.into(),
        };
        let params = S3ConnectionParams::new(provider, KEY_ID, Some(BUCKET)).expect("valid params");
        let mut profile = ProviderProfile::from_preset(&Preset::R2 {
            account_id: ACCOUNT.into(),
        })
        .expect("an R2 profile");
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
    let response = match (method.as_str(), key) {
        // Cut off: S3 publishes nothing.
        ("PUT", Some(_)) if body_len < length => return None,
        ("PUT", Some(key)) => {
            let etag = {
                let mut world = world.lock_ignore_poison();
                world.writes += 1;
                let etag = format!("\"v{}\"", world.writes);
                let meta = head
                    .lines()
                    .filter(|l| l.to_ascii_lowercase().starts_with("x-amz-meta-"))
                    .map(str::to_string)
                    .collect();
                world.objects.insert(
                    key,
                    Stored {
                        len: body_len,
                        etag: etag.clone(),
                        meta,
                    },
                );
                etag
            };
            tokio::time::sleep(answer_after).await;
            format!("HTTP/1.1 200 OK\r\netag: {etag}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
        }
        ("HEAD", Some(key)) => match world.lock_ignore_poison().objects.get(&key) {
            Some(stored) => format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\netag: {}\r\nlast-modified: Fri, 02 Oct 2026 10:00:00 GMT\r\n{}connection: close\r\n\r\n",
                stored.len,
                stored.etag,
                stored.meta.iter().map(|m| format!("{m}\r\n")).collect::<String>()
            ),
            None => NOT_FOUND.into(),
        },
        ("DELETE", Some(key)) => {
            world.lock_ignore_poison().objects.remove(&key);
            "HTTP/1.1 204 No Content\r\nconnection: close\r\n\r\n".into()
        }
        ("GET", None) if query.contains("list-type=2") => list(world, query),
        _ => "HTTP/1.1 501 Not Implemented\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".into(),
    };
    Some(response)
}

/// `ListObjectsV2`, every match as `Contents` (no delimiter folding), and ❗
/// B2's refusal of a prefix no key could carry.
fn list(world: &Mutex<World>, query: &str) -> String {
    let prefix = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("prefix="))
        .map(|value| percent_decode_str(value).decode_utf8_lossy().into_owned())
        .unwrap_or_default();
    let mut world = world.lock_ignore_poison();
    world.listed.push(prefix.clone());
    if prefix.len() > 1024 {
        let body = "<Error><Code>InvalidRequest</Code><Message>prefix too long</Message></Error>";
        return format!(
            "HTTP/1.1 400 Bad Request\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
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
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult><Name>{BUCKET}</Name><Prefix>{prefix}</Prefix><IsTruncated>false</IsTruncated>{contents}</ListBucketResult>"
    );
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/xml\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
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
