//! Fresh compression coordinator shutdown and stage-validation tests.

use super::*;
use crate::file_system::volume::{InMemoryVolume, ListingProgress, SpaceInfo};
use crate::file_system::write_operations::archive_edit::fresh_plan::FreshPlan;
use crate::file_system::write_operations::archive_edit::fresh_zip::{FreshZipEntry, FreshZipSource};

struct RefuseAfterProducerProgress {
    inner: InMemoryVolume,
    producer_progressed: Arc<tokio::sync::Notify>,
}

impl Volume for RefuseAfterProducerProgress {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn root(&self) -> &Path {
        self.inner.root()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn list_directory<'a>(
        &'a self,
        path: &'a Path,
        on_progress: Option<&'a (dyn Fn(ListingProgress) + Sync)>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<crate::file_system::listing::FileEntry>, VolumeError>> + Send + 'a>>
    {
        self.inner.list_directory(path, on_progress)
    }

    fn get_metadata<'a>(
        &'a self,
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<crate::file_system::listing::FileEntry, VolumeError>> + Send + 'a>> {
        self.inner.get_metadata(path)
    }

    fn exists<'a>(&'a self, path: &'a Path) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        self.inner.exists(path)
    }

    fn is_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<bool, VolumeError>> + Send + 'a>> {
        self.inner.is_directory(path)
    }

    fn get_space_info<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<SpaceInfo, VolumeError>> + Send + 'a>> {
        self.inner.get_space_info()
    }

    fn supports_unknown_length_writes(&self) -> bool {
        true
    }

    fn write_from_stream<'a>(
        &'a self,
        _dest: &'a Path,
        _mode: WriteMode,
        _length: StreamLength,
        stream: Box<dyn VolumeReadStream>,
        _on_progress: &'a (dyn Fn(StreamWriteProgress) -> ControlFlow<()> + Sync),
    ) -> Pin<Box<dyn Future<Output = Result<u64, VolumeError>> + Send + 'a>> {
        Box::pin(async move {
            self.producer_progressed.notified().await;
            drop(stream);
            Err(VolumeError::IoError {
                message: "injected destination refusal".to_string(),
                raw_os_error: None,
            })
        })
    }
}

#[tokio::test]
async fn backend_refusal_unblocks_a_producer_parked_by_pause() {
    let state = Arc::new(WriteOperationState::new(Duration::ZERO));
    state.pause_gate.pause();
    let producer_progressed = Arc::new(tokio::sync::Notify::new());
    let progress_signal = Arc::clone(&producer_progressed);
    let state_for_progress = Arc::clone(&state);
    let state_for_wake = Arc::clone(&state);
    let cancellation = FreshZipCancellation::new(Some(Arc::new(move || state_for_wake.pause_gate.wake())));
    let cancellation_for_progress = cancellation.clone();
    let progress: FreshZipProgressObserver = Arc::new(move |_| {
        progress_signal.notify_one();
        state_for_progress
            .pause_gate
            .wait_while_paused_sync_until(&|| cancellation_for_progress.is_requested());
        cancellation_for_progress.is_requested()
    });
    let plan = FreshPlan {
        source_volume: Arc::new(InMemoryVolume::new("source")),
        entries: vec![FreshZipEntry {
            name: "one.txt".to_string(),
            source: FreshZipSource::Bytes(b"one".to_vec()),
            size: 3,
            is_directory: false,
            modified: None,
            unix_mode: None,
        }],
        remote_feeds: Vec::new(),
        source_bytes: 3,
        skipped: 0,
    };
    let destination: Arc<dyn Volume> = Arc::new(RefuseAfterProducerProgress {
        inner: InMemoryVolume::new("destination"),
        producer_progressed,
    });

    let outcome = tokio::time::timeout(
        Duration::from_millis(250),
        produce_into(
            plan,
            destination,
            PathBuf::from("archive.zip"),
            StreamLength::Unknown,
            None,
            progress,
            cancellation,
            None,
        ),
    )
    .await;
    state.pause_gate.resume();

    assert!(
        outcome.is_ok(),
        "destination refusal must wake and join the paused producer"
    );
}

#[tokio::test]
async fn validation_rejects_byte_count_drift_before_parsing() {
    let volume: Arc<dyn Volume> = Arc::new(InMemoryVolume::new("stage"));
    volume
        .create_file(Path::new("archive.zip"), b"not a ZIP")
        .await
        .expect("create stage");

    assert!(
        validate_stage(&volume, Path::new("archive.zip"), 9, 8, 0)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn validation_rejects_corrupt_zip_bytes_even_when_counts_agree() {
    let volume: Arc<dyn Volume> = Arc::new(InMemoryVolume::new("stage"));
    let corrupt = b"PK\x03\x04truncated";
    volume
        .create_file(Path::new("archive.zip"), corrupt)
        .await
        .expect("create stage");

    let size = corrupt.len() as u64;
    assert!(
        validate_stage(&volume, Path::new("archive.zip"), size, size, 0)
            .await
            .is_err()
    );
}
