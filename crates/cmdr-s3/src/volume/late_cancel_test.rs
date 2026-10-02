//! A Cancel that arrives after a PUT's whole body went out, against a fake S3
//! that commits the object and then answers slowly (R2 did, live, 2026-10-02:
//! `live_hostile_cancel_uploads`). The publish can't be taken back by then, so
//! the write must report the file it finished, ❌ never `Cancelled` over an
//! object that's already replaced, and ❌ never remove it as a "cut-off" PUT.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cmdr_fs::volume::host::VolumeHost;
use cmdr_fs::volume::{Volume, VolumeError, VolumeReadStream, WriteMode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::S3Volume;
use super::testing::BytesSource;
use crate::params::{S3ConnectionParams, S3Provider};
use crate::profile::{Preset, ProviderProfile};
use crate::sigv4::Credentials;
use crate::transport::S3Client;

const ACCOUNT: &str = "acct";
const BUCKET: &str = "bucket";
const KEY_ID: &str = "AKIATEST";
const MIB: usize = 1024 * 1024;

/// One stored object: its length, ETag, and the `x-amz-meta-*` lines it was
/// written with.
#[derive(Clone)]
struct Stored {
    len: usize,
    etag: String,
    meta: Vec<String>,
}

#[derive(Default)]
struct World {
    objects: HashMap<String, Stored>,
    writes: usize,
}

/// A path-style S3 that answers HEAD, PUT, and DELETE, and ❗ commits a PUT
/// whose whole body arrived before waiting `answer_after` to say so.
struct SlowS3 {
    addr: SocketAddr,
    world: Arc<Mutex<World>>,
}

impl SlowS3 {
    async fn start(answer_after: Duration) -> Self {
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
                    let mut line = head.lines().next().unwrap_or_default().split(' ');
                    let method = line.next().unwrap_or_default().to_string();
                    let path = line.next().unwrap_or_default().to_string();
                    let response = match method.as_str() {
                        "PUT" if body_len < length => return, // cut off: nothing published
                        "PUT" => {
                            let etag = {
                                let mut world = world.lock().unwrap();
                                world.writes += 1;
                                let etag = format!("\"v{}\"", world.writes);
                                let meta = head
                                    .lines()
                                    .filter(|l| l.to_ascii_lowercase().starts_with("x-amz-meta-"))
                                    .map(str::to_string)
                                    .collect();
                                world.objects.insert(
                                    path,
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
                        "HEAD" => match world.lock().unwrap().objects.get(&path) {
                            Some(stored) => format!(
                                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\netag: {}\r\n{}connection: close\r\n\r\n",
                                stored.len,
                                stored.etag,
                                stored.meta.iter().map(|m| format!("{m}\r\n")).collect::<String>()
                            ),
                            None => "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".into(),
                        },
                        "DELETE" => {
                            world.lock().unwrap().objects.remove(&path);
                            "HTTP/1.1 204 No Content\r\nconnection: close\r\n\r\n".into()
                        }
                        _ => "HTTP/1.1 501 Not Implemented\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".into(),
                    };
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Self { addr, world }
    }

    fn object(&self, key: &str) -> Option<Stored> {
        self.world
            .lock()
            .unwrap()
            .objects
            .get(&format!("/{BUCKET}/{key}"))
            .cloned()
    }

    fn seed(&self, key: &str, len: usize) {
        self.world.lock().unwrap().objects.insert(
            format!("/{BUCKET}/{key}"),
            Stored {
                len,
                etag: "\"original\"".into(),
                meta: Vec::new(),
            },
        );
    }

    /// A bucket place on R2 (which refuses a short body, so an overwrite goes
    /// as one PUT), its endpoint dialing this fake over plain HTTP.
    fn volume(&self) -> S3Volume {
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

/// Writes `len` bytes to `key`, asking Cancel once every byte was handed over.
async fn write_cancelled_at_the_end(
    volume: &S3Volume,
    key: &str,
    mode: WriteMode,
    len: usize,
) -> Result<u64, VolumeError> {
    let source = BytesSource::new(vec![7u8; len]);
    let length = source.total_size();
    let total = len as u64;
    volume
        .write_from_stream(&volume.root().join(key), mode, length, Box::new(source), &|progress| {
            if progress.bytes_written >= total {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })
        .await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_after_the_whole_body_went_out_keeps_the_overwrite_it_finished() {
    let s3 = SlowS3::start(Duration::from_millis(700)).await;
    s3.seed("kept.bin", 32);
    let volume = s3.volume();
    let outcome = write_cancelled_at_the_end(&volume, "kept.bin", WriteMode::CreateOrReplace, 2 * MIB).await;
    let stored = s3.object("kept.bin");
    // Pre-fix: `Cancelled`, and the "cut-off" cleanup found our token on the
    // finished object and deleted it, so neither the original nor the new
    // bytes survived.
    assert_eq!(
        stored.map(|s| s.len),
        Some(2 * MIB),
        "the finished overwrite stays ({outcome:?})"
    );
    assert!(
        matches!(outcome, Ok(n) if n == (2 * MIB) as u64),
        "a publish that can't be taken back reports the file: {outcome:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_after_the_whole_body_went_out_keeps_the_new_file_it_finished() {
    let s3 = SlowS3::start(Duration::from_millis(700)).await;
    let volume = s3.volume();
    let outcome = write_cancelled_at_the_end(&volume, "fresh.bin", WriteMode::CreateOrReplace, 2 * MIB).await;
    assert!(matches!(outcome, Ok(n) if n == (2 * MIB) as u64), "{outcome:?}");
    assert_eq!(s3.object("fresh.bin").map(|s| s.len), Some(2 * MIB));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_before_the_last_piece_still_publishes_nothing() {
    let s3 = SlowS3::start(Duration::from_millis(700)).await;
    s3.seed("kept.bin", 32);
    let volume = s3.volume();
    let source = BytesSource::new(vec![7u8; 2 * MIB]);
    let length = source.total_size();
    let outcome = volume
        .write_from_stream(
            &volume.root().join("kept.bin"),
            WriteMode::CreateOrReplace,
            length,
            Box::new(source),
            &|progress| {
                if progress.bytes_written >= MIB as u64 {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            },
        )
        .await;
    assert!(matches!(outcome, Err(VolumeError::Cancelled(_))), "{outcome:?}");
    let stored = s3.object("kept.bin").expect("the original stays");
    assert_eq!((stored.len, stored.etag.as_str()), (32, "\"original\""));
}
