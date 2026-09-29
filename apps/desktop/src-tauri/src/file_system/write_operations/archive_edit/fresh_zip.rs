//! Seedless ZIP generation over bounded byte channels.
//!
//! `zip` 8.6's `ZipWriter::new_stream` writes data descriptors instead of seeking
//! back into local headers. `finish` writes the central directory and returns the
//! underlying writer; success is not inferred from output-channel EOF. ZIP64 is
//! selected per entry at `ZIP64_BYTES_THR` because stream mode cannot repair an
//! undersized local header after bytes have passed downstream (verified against
//! installed `zip-8.6.0/src/write.rs`, 2026-09-29).

#![allow(
    dead_code,
    reason = "the dedicated fresh-create driver consumes this reusable producer in the next change"
)]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{mpsc, oneshot};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const CHANNEL_CHUNKS: usize = 4;
const CHUNK_BYTES: usize = 128 * 1024;

pub(super) enum FreshZipSource {
    Local(PathBuf),
    Bytes(Vec<u8>),
    Remote(RemoteZipSource),
}

pub(super) struct RemoteZipSource {
    rx: std::sync::mpsc::Receiver<Result<Vec<u8>, FreshZipError>>,
}

pub(super) struct RemoteZipFeeder {
    tx: std::sync::mpsc::SyncSender<Result<Vec<u8>, FreshZipError>>,
}

pub(super) fn remote_source_bridge() -> (RemoteZipFeeder, RemoteZipSource) {
    let (tx, rx) = std::sync::mpsc::sync_channel(CHANNEL_CHUNKS);
    (RemoteZipFeeder { tx }, RemoteZipSource { rx })
}

impl RemoteZipFeeder {
    pub(super) async fn send(&self, chunk: Result<Vec<u8>, FreshZipError>) -> Result<(), FreshZipError> {
        let tx = self.tx.clone();
        tokio::task::spawn_blocking(move || tx.send(chunk))
            .await
            .map_err(|_| FreshZipError::ProducerPanicked)?
            .map_err(|_| FreshZipError::Cancelled)
    }
}

pub(super) struct FreshZipEntry {
    pub name: String,
    pub source: FreshZipSource,
    pub size: u64,
    pub is_directory: bool,
    pub modified: Option<std::time::SystemTime>,
    pub unix_mode: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FreshZipError {
    Cancelled,
    Source { entry: String, message: String },
    CountMismatch { entry: String, expected: u64, actual: u64 },
    Zip(String),
    OutputClosed,
    ProducerPanicked,
}

pub(super) struct FreshZipOutput {
    rx: mpsc::Receiver<Vec<u8>>,
    terminal: oneshot::Receiver<Result<u64, FreshZipError>>,
    join: Option<std::thread::JoinHandle<()>>,
    cancelled: Arc<AtomicBool>,
}

impl FreshZipOutput {
    pub(super) async fn next_chunk(&mut self) -> Option<Vec<u8>> {
        self.rx.recv().await
    }

    /// Waits for the explicit producer outcome and joins its blocking worker.
    /// A closed terminal channel is a producer failure, never successful EOF.
    pub(super) async fn finish(mut self) -> Result<u64, FreshZipError> {
        let terminal = (&mut self.terminal)
            .await
            .map_err(|_| FreshZipError::ProducerPanicked)?;
        if let Some(join) = self.join.take() {
            tokio::task::spawn_blocking(move || join.join())
                .await
                .map_err(|_| FreshZipError::ProducerPanicked)?
                .map_err(|_| FreshZipError::ProducerPanicked)?;
        }
        terminal
    }
}

impl Drop for FreshZipOutput {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.rx.close();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub(super) fn spawn_fresh_zip(entries: Vec<FreshZipEntry>, level: Option<i64>) -> FreshZipOutput {
    let (output_tx, output_rx) = mpsc::channel(CHANNEL_CHUNKS);
    let (terminal_tx, terminal_rx) = oneshot::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let join = std::thread::Builder::new()
        .name("fresh-zip-producer".into())
        .spawn(move || {
            let result = produce(entries, level, output_tx, &worker_cancelled);
            let _ = terminal_tx.send(result);
        })
        .expect("the OS can create the bounded ZIP producer worker");
    FreshZipOutput {
        rx: output_rx,
        terminal: terminal_rx,
        join: Some(join),
        cancelled,
    }
}

struct ChannelWriter {
    tx: mpsc::Sender<Vec<u8>>,
    pending: Vec<u8>,
    written: u64,
    cancelled: Arc<AtomicBool>,
}

impl ChannelWriter {
    fn flush_pending(&mut self) -> std::io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        if self.cancelled.load(Ordering::Acquire) {
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
        }
        let chunk = std::mem::replace(&mut self.pending, Vec::with_capacity(CHUNK_BYTES));
        self.tx
            .blocking_send(chunk)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "ZIP destination closed"))
    }
}

impl Write for ChannelWriter {
    fn write(&mut self, mut buf: &[u8]) -> std::io::Result<usize> {
        let original = buf.len();
        while !buf.is_empty() {
            let room = CHUNK_BYTES - self.pending.len();
            let take = room.min(buf.len());
            self.pending.extend_from_slice(&buf[..take]);
            self.written += take as u64;
            buf = &buf[take..];
            if self.pending.len() == CHUNK_BYTES {
                self.flush_pending()?;
            }
        }
        Ok(original)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.flush_pending()
    }
}

fn produce(
    entries: Vec<FreshZipEntry>,
    level: Option<i64>,
    tx: mpsc::Sender<Vec<u8>>,
    cancelled: &Arc<AtomicBool>,
) -> Result<u64, FreshZipError> {
    let output = ChannelWriter {
        tx,
        pending: Vec::with_capacity(CHUNK_BYTES),
        written: 0,
        cancelled: Arc::clone(cancelled),
    };
    let mut zip = ZipWriter::new_stream(output);
    for mut entry in entries {
        checkpoint(cancelled)?;
        let mut options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .compression_level(level.map(|value| value.clamp(1, 9)))
            .large_file(needs_zip64(entry.size));
        if let Some(mode) = entry.unix_mode {
            options = options.unix_permissions(mode);
        }
        if let Some(modified) = entry.modified.and_then(system_time_to_zip_datetime) {
            options = options.last_modified_time(modified);
        }
        if entry.is_directory {
            zip.add_directory(entry.name.trim_end_matches('/'), options)
                .map_err(map_zip)?;
            continue;
        }
        zip.start_file(entry.name.clone(), options).map_err(map_zip)?;
        let actual = match &mut entry.source {
            FreshZipSource::Local(path) => {
                let mut file = std::fs::File::open(path).map_err(|error| FreshZipError::Source {
                    entry: entry.name.clone(),
                    message: error.to_string(),
                })?;
                copy_reader(&mut file, &mut zip, &entry.name, cancelled)?
            }
            FreshZipSource::Bytes(bytes) => copy_reader(&mut bytes.as_slice(), &mut zip, &entry.name, cancelled)?,
            FreshZipSource::Remote(source) => {
                let mut actual = 0;
                loop {
                    checkpoint(cancelled)?;
                    match source.rx.recv_timeout(std::time::Duration::from_millis(20)) {
                        Ok(Ok(bytes)) => {
                            actual += bytes.len() as u64;
                            zip.write_all(&bytes).map_err(map_io)?;
                        }
                        Ok(Err(error)) => return Err(error),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                actual
            }
        };
        if actual != entry.size {
            return Err(FreshZipError::CountMismatch {
                entry: entry.name,
                expected: entry.size,
                actual,
            });
        }
    }
    let mut output = zip.finish().map_err(map_zip)?.into_inner();
    output.flush_pending().map_err(map_io)?;
    Ok(output.written)
}

fn copy_reader(
    reader: &mut dyn Read,
    output: &mut impl Write,
    entry: &str,
    cancelled: &Arc<AtomicBool>,
) -> Result<u64, FreshZipError> {
    let mut buffer = [0u8; CHUNK_BYTES];
    let mut total = 0;
    loop {
        checkpoint(cancelled)?;
        let count = reader.read(&mut buffer).map_err(|error| FreshZipError::Source {
            entry: entry.to_string(),
            message: error.to_string(),
        })?;
        if count == 0 {
            return Ok(total);
        }
        total += count as u64;
        output.write_all(&buffer[..count]).map_err(map_io)?;
    }
}

fn checkpoint(cancelled: &AtomicBool) -> Result<(), FreshZipError> {
    if cancelled.load(Ordering::Acquire) {
        Err(FreshZipError::Cancelled)
    } else {
        Ok(())
    }
}

fn needs_zip64(size: u64) -> bool {
    size >= zip::ZIP64_BYTES_THR
}

fn map_zip(error: zip::result::ZipError) -> FreshZipError {
    FreshZipError::Zip(error.to_string())
}

fn map_io(error: std::io::Error) -> FreshZipError {
    match error.kind() {
        std::io::ErrorKind::Interrupted => FreshZipError::Cancelled,
        std::io::ErrorKind::BrokenPipe => FreshZipError::OutputClosed,
        _ => FreshZipError::Zip(error.to_string()),
    }
}

fn system_time_to_zip_datetime(time: std::time::SystemTime) -> Option<zip::DateTime> {
    use chrono::{Datelike, Timelike};
    let dt: chrono::DateTime<chrono::Utc> = time.into();
    let date = (dt.year().try_into().ok()?, dt.month() as u8, dt.day() as u8);
    let clock = (dt.hour() as u8, dt.minute() as u8, dt.second() as u8);
    zip::DateTime::from_date_and_time(date.0, date.1, date.2, clock.0, clock.1, clock.2).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn collect(mut output: FreshZipOutput) -> Result<Vec<u8>, FreshZipError> {
        let mut bytes = Vec::new();
        while let Some(chunk) = output.next_chunk().await {
            bytes.extend(chunk);
        }
        output.finish().await?;
        Ok(bytes)
    }

    #[test]
    fn zip64_is_selected_at_the_per_entry_boundary() {
        assert!(!needs_zip64(zip::ZIP64_BYTES_THR - 1));
        assert!(needs_zip64(zip::ZIP64_BYTES_THR));
        assert!(needs_zip64(zip::ZIP64_BYTES_THR + 1));
    }

    #[tokio::test]
    async fn producer_preserves_names_empty_entries_metadata_and_level() {
        let entries = vec![
            FreshZipEntry {
                name: "folder/".into(),
                source: FreshZipSource::Bytes(vec![]),
                size: 0,
                is_directory: true,
                modified: None,
                unix_mode: Some(0o750),
            },
            FreshZipEntry {
                name: "folder/empty.txt".into(),
                source: FreshZipSource::Bytes(vec![]),
                size: 0,
                is_directory: false,
                modified: None,
                unix_mode: Some(0o640),
            },
        ];
        let bytes = collect(spawn_fresh_zip(entries, Some(1))).await.expect("produce");
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("valid zip");
        assert_eq!(archive.len(), 2);
        assert_eq!(archive.by_index(0).expect("dir").name(), "folder/");
        assert_eq!(archive.by_index(0).expect("dir").unix_mode(), Some(0o40750));
        assert_eq!(archive.by_index(1).expect("file").unix_mode(), Some(0o100640));
        assert_eq!(archive.by_index(1).expect("file").size(), 0);
    }

    #[tokio::test]
    async fn output_larger_than_capacity_waits_for_a_gated_consumer() {
        let payload = (0..CHUNK_BYTES * (CHANNEL_CHUNKS + 3))
            .map(|n| (n % 251) as u8)
            .collect::<Vec<_>>();
        let output = spawn_fresh_zip(
            vec![FreshZipEntry {
                name: "large.bin".into(),
                size: payload.len() as u64,
                source: FreshZipSource::Bytes(payload.clone()),
                is_directory: false,
                modified: None,
                unix_mode: None,
            }],
            None,
        );
        tokio::task::yield_now().await;
        let bytes = collect(output)
            .await
            .expect("producer resumes after consumer opens gate");
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("valid zip");
        let mut actual = Vec::new();
        archive
            .by_name("large.bin")
            .expect("entry")
            .read_to_end(&mut actual)
            .expect("read");
        assert_eq!(actual, payload);
    }

    #[tokio::test]
    async fn late_remote_source_error_is_not_eof() {
        let (feeder, source) = remote_source_bridge();
        let output = spawn_fresh_zip(
            vec![FreshZipEntry {
                name: "remote.bin".into(),
                size: 6,
                source: FreshZipSource::Remote(source),
                is_directory: false,
                modified: None,
                unix_mode: None,
            }],
            None,
        );
        feeder.send(Ok(b"prefix".to_vec())).await.expect("first chunk");
        feeder
            .send(Err(FreshZipError::Source {
                entry: "remote.bin".into(),
                message: "late read refusal".into(),
            }))
            .await
            .expect("late error");
        drop(feeder);
        assert!(matches!(collect(output).await, Err(FreshZipError::Source { .. })));
    }

    #[tokio::test]
    async fn planned_source_count_mismatch_is_terminal_failure() {
        let output = spawn_fresh_zip(
            vec![FreshZipEntry {
                name: "drifted.bin".into(),
                size: 8,
                source: FreshZipSource::Bytes(b"short".to_vec()),
                is_directory: false,
                modified: None,
                unix_mode: None,
            }],
            None,
        );
        assert!(matches!(
            collect(output).await,
            Err(FreshZipError::CountMismatch {
                expected: 8,
                actual: 5,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn dropping_destination_cancels_a_backpressured_producer() {
        let output = spawn_fresh_zip(
            vec![FreshZipEntry {
                name: "large.bin".into(),
                size: (CHUNK_BYTES * 20) as u64,
                source: FreshZipSource::Bytes(vec![7; CHUNK_BYTES * 20]),
                is_directory: false,
                modified: None,
                unix_mode: None,
            }],
            None,
        );
        drop(output);
    }
}
