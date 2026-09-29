//! Fresh-ZIP source planning and source/destination identity checks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use super::super::OperationEventSink;
use super::super::state::WriteOperationState;
use super::super::types::{ConflictResolution, WriteConflictEvent, WriteConflictResolvedEvent, WriteOperationError};
use super::edit_error::EditError;
use super::fresh_zip::{FreshZipEntry, FreshZipSource, RemoteZipFeeder, remote_source_bridge};
use crate::file_system::volume::{Volume, VolumeError};

pub(super) struct RemoteFeed {
    pub(super) path: PathBuf,
    pub(super) entry_name: String,
    pub(super) feeder: RemoteZipFeeder,
}

pub(super) struct FreshPlan {
    pub(super) source_volume: Arc<dyn Volume>,
    pub(super) entries: Vec<FreshZipEntry>,
    pub(super) remote_feeds: Vec<RemoteFeed>,
    pub(super) source_bytes: u64,
    pub(super) skipped: usize,
}

#[derive(Clone, Copy)]
struct PlanConflictContext<'a> {
    events: &'a dyn OperationEventSink,
    operation_id: &'a str,
    archive_path: &'a Path,
}

pub(super) async fn plan_sources(
    source: &Arc<dyn Volume>,
    source_paths: &[PathBuf],
    conflict: ConflictResolution,
    state: &Arc<WriteOperationState>,
    events: &dyn OperationEventSink,
    operation_id: &str,
    archive_path: &Path,
) -> Result<FreshPlan, EditError> {
    plan_sources_with_context(
        source,
        source_paths,
        conflict,
        state,
        Some(PlanConflictContext {
            events,
            operation_id,
            archive_path,
        }),
    )
    .await
}

async fn plan_sources_with_context(
    source: &Arc<dyn Volume>,
    source_paths: &[PathBuf],
    conflict: ConflictResolution,
    state: &Arc<WriteOperationState>,
    conflict_context: Option<PlanConflictContext<'_>>,
) -> Result<FreshPlan, EditError> {
    let mut entries: Vec<FreshZipEntry> = Vec::new();
    let mut remote_feeds: Vec<RemoteFeed> = Vec::new();
    let mut source_bytes = 0u64;
    let mut skipped = 0usize;
    let mut top_names = HashMap::<String, PathBuf>::new();
    let mut duplicate_latch = None;
    let local_root = source.supports_local_fs_access().then(|| source.local_path()).flatten();

    for top in source_paths {
        if state.stop_or_park_async().await {
            return Err(EditError::Cancelled);
        }
        let Some(mut top_name) = top.file_name().map(|name| name.to_string_lossy().into_owned()) else {
            skipped += 1;
            continue;
        };
        if let Some(first) = top_names.get(&top_name).cloned() {
            let resolution = if let Some(latched) = duplicate_latch {
                latched
            } else if conflict == ConflictResolution::Stop {
                let Some(context) = conflict_context else {
                    return Err(EditError::Op(WriteOperationError::DuplicateSourceNames {
                        name: top_name,
                        first: first.display().to_string(),
                        second: top.display().to_string(),
                    }));
                };
                let response = prompt_duplicate_source(source, top, &first, &top_name, state, context).await?;
                if response.apply_to_all {
                    duplicate_latch = Some(response.resolution);
                }
                response.resolution
            } else {
                conflict
            };
            let resolution = reduce_duplicate_conditional(source, top, &first, resolution).await;
            match resolution {
                ConflictResolution::Skip
                | ConflictResolution::OverwriteSmaller
                | ConflictResolution::OverwriteOlder => {
                    skipped += 1;
                    continue;
                }
                ConflictResolution::Stop => {
                    return Err(EditError::Op(WriteOperationError::DuplicateSourceNames {
                        name: top_name,
                        first: first.display().to_string(),
                        second: top.display().to_string(),
                    }));
                }
                ConflictResolution::Rename => top_name = unique_top_name(&top_name, &top_names),
                ConflictResolution::Overwrite => {
                    let prefix = format!("{top_name}/");
                    entries.retain(|entry| entry.name != top_name && !entry.name.starts_with(&prefix));
                    remote_feeds.retain(|feed| feed.entry_name != top_name && !feed.entry_name.starts_with(&prefix));
                    source_bytes = entries.iter().map(|entry| entry.size).sum();
                }
            }
        }
        top_names.insert(top_name.clone(), top.clone());
        let mut stack = vec![(top.clone(), top_name)];
        while let Some((path, inner)) = stack.pop() {
            if state.stop_or_park_async().await {
                return Err(EditError::Cancelled);
            }
            let meta = source
                .get_metadata(&path)
                .await
                .map_err(|error| volume_read_error(&path, error))?;
            if meta.is_symlink {
                skipped += 1;
                continue;
            }
            if let Some(root) = &local_root {
                let local_path = if path.is_absolute() {
                    path.clone()
                } else {
                    root.join(&path)
                };
                let is_special = tokio::task::spawn_blocking(move || {
                    std::fs::symlink_metadata(local_path)
                        .is_ok_and(|metadata| !metadata.file_type().is_file() && !metadata.file_type().is_dir())
                })
                .await
                .map_err(|error| {
                    EditError::Op(WriteOperationError::ReadError {
                        path: path.display().to_string(),
                        message: error.to_string(),
                    })
                })?;
                if is_special {
                    skipped += 1;
                    continue;
                }
            }
            let modified = meta
                .modified_at
                .map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds));
            let unix_mode = source
                .reports_posix_mode()
                .then_some(meta.permissions)
                .filter(|mode| *mode != 0);
            if meta.is_directory {
                entries.push(FreshZipEntry {
                    name: format!("{}/", inner.trim_end_matches('/')),
                    source: FreshZipSource::Bytes(Vec::new()),
                    size: 0,
                    is_directory: true,
                    modified,
                    unix_mode,
                });
                let children = source
                    .list_directory(&path, None)
                    .await
                    .map_err(|error| volume_read_error(&path, error))?;
                for child in children.into_iter().rev() {
                    stack.push((path.join(&child.name), format!("{inner}/{}", child.name)));
                }
            } else {
                let size = meta.size.ok_or_else(|| {
                    EditError::Op(WriteOperationError::ReadError {
                        path: path.display().to_string(),
                        message: "the source did not report its size".to_string(),
                    })
                })?;
                source_bytes = source_bytes.saturating_add(size);
                let entry_source = if let Some(root) = &local_root {
                    FreshZipSource::Local(if path.is_absolute() {
                        path.clone()
                    } else {
                        root.join(&path)
                    })
                } else {
                    let (feeder, remote) = remote_source_bridge();
                    remote_feeds.push(RemoteFeed {
                        path: path.clone(),
                        entry_name: inner.clone(),
                        feeder,
                    });
                    FreshZipSource::Remote(remote)
                };
                entries.push(FreshZipEntry {
                    name: inner,
                    source: entry_source,
                    size,
                    is_directory: false,
                    modified,
                    unix_mode,
                });
            }
        }
    }
    Ok(FreshPlan {
        source_volume: Arc::clone(source),
        entries,
        remote_feeds,
        source_bytes,
        skipped,
    })
}

async fn prompt_duplicate_source(
    source: &Arc<dyn Volume>,
    incoming: &Path,
    existing: &Path,
    name: &str,
    state: &Arc<WriteOperationState>,
    context: PlanConflictContext<'_>,
) -> Result<super::super::state::ConflictResolutionResponse, EditError> {
    let incoming_meta = source.get_metadata(incoming).await.ok();
    let existing_meta = source.get_metadata(existing).await.ok();
    let source_size = incoming_meta.as_ref().and_then(|meta| meta.size);
    let destination_size = existing_meta.as_ref().and_then(|meta| meta.size);
    let source_modified = incoming_meta
        .as_ref()
        .and_then(|meta| meta.modified_at)
        .and_then(|value| i64::try_from(value).ok());
    let destination_modified = existing_meta
        .as_ref()
        .and_then(|meta| meta.modified_at)
        .and_then(|value| i64::try_from(value).ok());
    let (tx, rx) = tokio::sync::oneshot::channel();
    let event = state.conflict_slot.arm(tx, |conflict_id| WriteConflictEvent {
        operation_id: context.operation_id.to_string(),
        conflict_id,
        source_path: incoming.display().to_string(),
        destination_path: context.archive_path.join(name).display().to_string(),
        source_size,
        destination_size,
        source_modified,
        destination_modified,
        destination_is_newer: matches!((source_modified, destination_modified), (Some(source), Some(dest)) if dest > source),
        size_difference: match (destination_size, source_size) {
            (Some(dest), Some(source)) => i64::try_from(dest)
                .ok()
                .zip(i64::try_from(source).ok())
                .map(|(dest, source)| dest - source),
            _ => None,
        },
        source_is_directory: incoming_meta.as_ref().is_some_and(|meta| meta.is_directory),
        destination_is_directory: existing_meta.as_ref().is_some_and(|meta| meta.is_directory),
        destination_is_look_alike: false,
    });
    let conflict_id = event.conflict_id;
    state.announce_human_wait(context.events);
    context.events.emit_conflict(event);
    let response = rx.await.map_err(|_| EditError::Cancelled)?;
    state.announce_human_wait(context.events);
    context.events.emit_conflict_resolved(WriteConflictResolvedEvent {
        operation_id: context.operation_id.to_string(),
        conflict_id,
    });
    Ok(response)
}

async fn reduce_duplicate_conditional(
    source: &Arc<dyn Volume>,
    incoming: &Path,
    existing: &Path,
    resolution: ConflictResolution,
) -> ConflictResolution {
    let (Ok(incoming), Ok(existing)) = (source.get_metadata(incoming).await, source.get_metadata(existing).await)
    else {
        return match resolution {
            ConflictResolution::OverwriteSmaller | ConflictResolution::OverwriteOlder => ConflictResolution::Skip,
            other => other,
        };
    };
    match resolution {
        ConflictResolution::OverwriteSmaller => match (incoming.size, existing.size) {
            (Some(incoming), Some(existing)) if existing < incoming => ConflictResolution::Overwrite,
            _ => ConflictResolution::Skip,
        },
        ConflictResolution::OverwriteOlder => match (incoming.modified_at, existing.modified_at) {
            (Some(incoming), Some(existing)) if existing < incoming => ConflictResolution::Overwrite,
            _ => ConflictResolution::Skip,
        },
        other => other,
    }
}

fn unique_top_name(name: &str, planned: &HashMap<String, PathBuf>) -> String {
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|part| part.to_str()).unwrap_or(name);
    let extension = path
        .extension()
        .and_then(|part| part.to_str())
        .map(|part| format!(".{part}"))
        .unwrap_or_default();
    for suffix in 1..=9999 {
        let candidate = format!("{stem} ({suffix}){extension}");
        if !planned.contains_key(&candidate) {
            return candidate;
        }
    }
    format!("{stem} ({}){extension}", uuid::Uuid::new_v4())
}

pub(super) async fn validate_aliases(
    source: &Arc<dyn Volume>,
    source_paths: &[PathBuf],
    dest: &Arc<dyn Volume>,
    archive_path: &Path,
) -> Result<(), WriteOperationError> {
    let source_local = source.supports_local_fs_access().then(|| source.local_path()).flatten();
    let dest_local = dest.supports_local_fs_access().then(|| dest.local_path()).flatten();
    if let (Some(root), Some(dest_root)) = (source_local, dest_local) {
        let absolute_sources = source_paths
            .iter()
            .map(|path| {
                if path.is_absolute() {
                    path.clone()
                } else {
                    root.join(path)
                }
            })
            .collect::<Vec<_>>();
        let absolute_dest = if archive_path.is_absolute() {
            archive_path.to_path_buf()
        } else {
            dest_root.join(archive_path)
        };
        return tokio::task::spawn_blocking(move || {
            super::super::validation::validate_destination_not_inside_source(&absolute_sources, &absolute_dest)?;
            for source_path in &absolute_sources {
                if super::super::validation::is_same_file(source_path, &absolute_dest) {
                    return Err(WriteOperationError::DestinationInsideSource {
                        source: source_path.display().to_string(),
                        destination: absolute_dest.display().to_string(),
                    });
                }
            }
            Ok(())
        })
        .await
        .map_err(|error| WriteOperationError::IoError {
            path: archive_path.display().to_string(),
            message: error.to_string(),
        })?;
    } else if Arc::ptr_eq(source, dest) || source.lane_key() == dest.lane_key() {
        let archive_path = cmdr_fs::volume::remote_paths::normalize_remote_path(archive_path);
        for source_path in source_paths {
            let source_path = cmdr_fs::volume::remote_paths::normalize_remote_path(source_path);
            let meta = source.get_metadata(&source_path).await.ok();
            if source_path == archive_path
                || meta.is_some_and(|entry| entry.is_directory && archive_path.starts_with(&source_path))
            {
                return Err(WriteOperationError::DestinationInsideSource {
                    source: source_path.display().to_string(),
                    destination: archive_path.display().to_string(),
                });
            }
        }
    }
    Ok(())
}

fn volume_read_error(path: &Path, error: VolumeError) -> EditError {
    EditError::Op(WriteOperationError::ReadError {
        path: path.display().to_string(),
        message: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_system::volume::{InMemoryVolume, LocalPosixVolume};

    #[tokio::test]
    async fn distinct_handles_for_one_remote_resource_still_reject_containment() {
        let lane = "same-remote-resource";
        let source: Arc<dyn Volume> = Arc::new(InMemoryVolume::new("source").with_lane_key(lane));
        let destination: Arc<dyn Volume> = Arc::new(InMemoryVolume::new("destination").with_lane_key(lane));
        source
            .create_directory(Path::new("/folder"))
            .await
            .expect("create source folder");

        assert!(matches!(
            validate_aliases(
                &source,
                &[PathBuf::from("/folder")],
                &destination,
                Path::new("/folder/archive.zip"),
            )
            .await,
            Err(WriteOperationError::DestinationInsideSource { .. })
        ));
    }

    #[tokio::test]
    async fn duplicate_top_names_honor_rename_and_skip_policies() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join("a")).expect("create a");
        std::fs::create_dir_all(temp.path().join("b")).expect("create b");
        std::fs::write(temp.path().join("a/report.txt"), b"first").expect("write first");
        std::fs::write(temp.path().join("b/report.txt"), b"second").expect("write second");
        let source: Arc<dyn Volume> = Arc::new(LocalPosixVolume::new("source", temp.path().to_path_buf()));
        let paths = [PathBuf::from("a/report.txt"), PathBuf::from("b/report.txt")];
        let state = Arc::new(WriteOperationState::new(Duration::ZERO));

        let renamed = plan_sources_with_context(&source, &paths, ConflictResolution::Rename, &state, None)
            .await
            .unwrap_or_else(|_| panic!("rename plan"));
        assert_eq!(
            renamed
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["report.txt", "report (1).txt"]
        );

        let skipped = plan_sources_with_context(&source, &paths, ConflictResolution::Skip, &state, None)
            .await
            .unwrap_or_else(|_| panic!("skip plan"));
        assert_eq!(skipped.entries.len(), 1);
        assert_eq!(skipped.skipped, 1);
    }
}
