//! The record of every multipart upload Cmdr started and hasn't finished or
//! aborted, so a later connect can abort the leftovers: S3 keeps an unfinished
//! upload's parts forever, invisible in every listing, and bills them.
//!
//! ❗ **Why a local record and not the server's list.** `ListMultipartUploads`
//! answers key, upload id, initiator, and time, and nothing marks an upload as
//! Cmdr's: aborting by age or prefix would also abort another tool's live
//! upload. So the sweep aborts exactly the upload ids written here, ❌ never
//! one it didn't record (`DETAILS.md` § "Unfinished uploads").
//!
//! ❗ **An upload running in this process is never a leftover**, even to a
//! second place of the same account that connects mid-upload: the in-flight
//! set is process-wide for that reason.
//!
//! It also records each temp object an overwrite writes first (`writes.rs` §
//! "Overwrites through a temp key"): a crash between that temp's PUT and its
//! removal leaves an object under a `.cmdr-tmp-` name, and on a server that
//! publishes a cut-off PUT a truncated one. The sweep deletes such a temp only
//! while it still carries the write's own token.
//!
//! The log is `<state dir>/unfinished-uploads` (`VolumeHost::state_dir`), one
//! line per event: `+ <account> <bucket> <key> <upload id>` when an upload
//! starts, `- …` when it completes or is aborted, and `T` / `t` with the
//! write's token in place of the upload id for a temp object, every field
//! percent-encoded so a key holding a space or a line break can't break a
//! line. Reading leftovers rewrites it to the records still open. Without a
//! state directory the records live in memory for the session. Every access
//! takes one process-wide lock: the events come at human cadence (two per
//! upload), and one lock keeps a rewrite from losing an append.

use std::collections::HashSet;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use cmdr_fs::ignore_poison::IgnorePoison;
use log::warn;
use percent_encoding::percent_decode_str;

use crate::encoding::encode_component;

/// The log's file name inside the backend's state directory.
const LOG_NAME: &str = "unfinished-uploads";

/// One upload Cmdr started and hasn't finished or aborted yet.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct UnfinishedUpload {
    /// The account that can abort it (`S3VolumeInner::account`): the endpoint
    /// plus the access key id.
    pub account: String,
    pub bucket: String,
    pub key: String,
    pub upload_id: String,
}

/// One temp object an overwrite wrote (or is writing) before the copy that
/// replaces the original, and the token its PUT carried.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct TempObject {
    /// As on [`UnfinishedUpload`].
    pub account: String,
    pub bucket: String,
    pub key: String,
    /// The `x-amz-meta-cmdr-write` token: the sweep deletes the key only
    /// while the object there still carries it.
    pub token: String,
}

/// What the whole process knows that the log doesn't.
#[derive(Default)]
struct Registry {
    /// Uploads an upload task in this process is running right now.
    in_flight: HashSet<UnfinishedUpload>,
    /// Records not yet finished, for this session: the whole ledger without a
    /// state directory, and a backstop beside the log with one (an append
    /// that failed still reaches this session's sweeps).
    open: HashSet<UnfinishedUpload>,
    /// The same two sets for temp objects.
    temps_in_flight: HashSet<TempObject>,
    temps_open: HashSet<TempObject>,
}

/// One line of the log, either kind.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Record {
    Upload(UnfinishedUpload),
    Temp(TempObject),
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

fn registry() -> MutexGuard<'static, Option<Registry>> {
    REGISTRY.lock_ignore_poison()
}

/// One volume's handle on the ledger: where its log lives, if anywhere.
#[derive(Debug, Clone)]
pub(super) struct UploadLedger {
    log: Option<PathBuf>,
}

impl UploadLedger {
    /// The ledger under `dir` (`VolumeHost::state_dir`), or a session-only one.
    pub(super) fn at(dir: Option<PathBuf>) -> Self {
        Self {
            log: dir.map(|dir| dir.join(LOG_NAME)),
        }
    }

    /// `upload` exists on the server now and an upload task is running it.
    /// ❗ Called right after `CreateMultipartUpload` answers, before any part.
    pub(super) fn started(&self, upload: &UnfinishedUpload) {
        let mut guard = registry();
        let registry = guard.get_or_insert_with(Registry::default);
        registry.in_flight.insert(upload.clone());
        registry.open.insert(upload.clone());
        self.append('+', upload);
    }

    /// `upload` completed or was aborted: nothing of it is left to sweep.
    pub(super) fn finished(&self, upload: &UnfinishedUpload) {
        let mut guard = registry();
        let registry = guard.get_or_insert_with(Registry::default);
        registry.in_flight.remove(upload);
        registry.open.remove(upload);
        self.append('-', upload);
    }

    /// The task running `upload` ended without completing or aborting it (an
    /// abort the server never answered): keep the record for the next sweep,
    /// and stop counting it as in flight.
    pub(super) fn abandoned(&self, upload: &UnfinishedUpload) {
        if let Some(registry) = registry().as_mut() {
            registry.in_flight.remove(upload);
        }
    }

    /// What a sweep for `account` may abort: every open record that no task in
    /// this process is running. Rewrites the log to the records still open.
    pub(super) fn leftovers(&self, account: &str) -> Vec<UnfinishedUpload> {
        let mut guard = registry();
        let registry = guard.get_or_insert_with(Registry::default);
        let mut open: Vec<UnfinishedUpload> = self
            .replay_and_compact()
            .into_iter()
            .filter_map(|record| match record {
                Record::Upload(upload) => Some(upload),
                Record::Temp(_) => None,
            })
            .collect();
        for record in &registry.open {
            if !open.contains(record) {
                open.push(record.clone());
            }
        }
        open.retain(|record| record.account == account && !registry.in_flight.contains(record));
        open
    }

    /// `temp` is about to be written: recorded before its PUT, so a crash
    /// mid-write still leaves a record.
    pub(super) fn temp_started(&self, temp: &TempObject) {
        let mut guard = registry();
        let registry = guard.get_or_insert_with(Registry::default);
        registry.temps_in_flight.insert(temp.clone());
        registry.temps_open.insert(temp.clone());
        self.append_line(&temp_line('T', temp));
    }

    /// `temp` is gone from the server (deleted, or never written).
    pub(super) fn temp_finished(&self, temp: &TempObject) {
        let mut guard = registry();
        let registry = guard.get_or_insert_with(Registry::default);
        registry.temps_in_flight.remove(temp);
        registry.temps_open.remove(temp);
        self.append_line(&temp_line('t', temp));
    }

    /// The write owning `temp` ended without removing it: keep the record for
    /// the next sweep.
    pub(super) fn temp_abandoned(&self, temp: &TempObject) {
        if let Some(registry) = registry().as_mut() {
            registry.temps_in_flight.remove(temp);
        }
    }

    /// The temp objects a sweep for `account` may remove: every open record no
    /// write in this process owns. Rewrites the log to the records still open.
    pub(super) fn temp_leftovers(&self, account: &str) -> Vec<TempObject> {
        let mut guard = registry();
        let registry = guard.get_or_insert_with(Registry::default);
        let mut open: Vec<TempObject> = self
            .replay_and_compact()
            .into_iter()
            .filter_map(|record| match record {
                Record::Temp(temp) => Some(temp),
                Record::Upload(_) => None,
            })
            .collect();
        for record in &registry.temps_open {
            if !open.contains(record) {
                open.push(record.clone());
            }
        }
        open.retain(|record| record.account == account && !registry.temps_in_flight.contains(record));
        open
    }

    /// Every record the log holds open, both kinds, after rewriting it to
    /// just those. Called with the registry lock held, like every access.
    fn replay_and_compact(&self) -> Vec<Record> {
        match &self.log {
            Some(log) => {
                let recorded = replay(log);
                compact(log, &recorded);
                recorded
            }
            None => Vec::new(),
        }
    }

    /// Every open upload record of `account` whose key starts with `prefix`,
    /// in flight or not. A fixture cell asks it about its own scratch prefix:
    /// the registry is process-wide, so an unscoped question would see
    /// another cell's records of the same account.
    #[cfg(test)]
    pub(super) fn open_under(&self, account: &str, prefix: &str) -> Vec<UnfinishedUpload> {
        let mut guard = registry();
        let registry = guard.get_or_insert_with(Registry::default);
        let mut open: Vec<UnfinishedUpload> = self
            .log
            .as_deref()
            .map(replay)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|record| match record {
                Record::Upload(upload) => Some(upload),
                Record::Temp(_) => None,
            })
            .collect();
        for record in registry.open.iter().chain(&registry.in_flight) {
            if !open.contains(record) {
                open.push(record.clone());
            }
        }
        open.retain(|record| record.account == account && record.key.starts_with(prefix));
        open
    }

    /// What a crash does to a record: the in-flight mark goes with the process
    /// and only the log remains.
    #[cfg(test)]
    pub(super) fn forget_in_flight(upload: &UnfinishedUpload) {
        if let Some(registry) = registry().as_mut() {
            registry.in_flight.remove(upload);
            registry.open.remove(upload);
        }
    }

    /// What a crash does to a temp's record, as [`Self::forget_in_flight`].
    #[cfg(test)]
    pub(super) fn forget_temp_in_flight(temp: &TempObject) {
        if let Some(registry) = registry().as_mut() {
            registry.temps_in_flight.remove(temp);
            registry.temps_open.remove(temp);
        }
    }

    /// Appends one event to the log. A failure is logged and survived: the
    /// session's own record still holds it, and a lost line only costs a
    /// leftover a later launch won't know about.
    fn append(&self, op: char, upload: &UnfinishedUpload) {
        self.append_line(&line(op, upload));
    }

    fn append_line(&self, text: &str) {
        let Some(log) = &self.log else {
            return;
        };
        let written = log
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::OpenOptions::new().create(true).append(true).open(log))
            .and_then(|mut file| file.write_all(text.as_bytes()));
        if let Err(e) = written {
            warn!(target: "volume", "s3: couldn't record an upload in {}: {e}", log.display());
        }
    }
}

/// One log line for `upload`.
fn line(op: char, upload: &UnfinishedUpload) -> String {
    format!(
        "{op} {} {} {} {}\n",
        encode_component(&upload.account),
        encode_component(&upload.bucket),
        encode_component(&upload.key),
        encode_component(&upload.upload_id)
    )
}

/// One log line for a temp object: `T` when written, `t` when gone.
fn temp_line(op: char, temp: &TempObject) -> String {
    format!(
        "{op} {} {} {} {}\n",
        encode_component(&temp.account),
        encode_component(&temp.bucket),
        encode_component(&temp.key),
        encode_component(&temp.token)
    )
}

/// The records the log holds open: every `+` without a later `-` and every
/// `T` without a later `t`, in the order they started. A line that doesn't
/// read is skipped.
fn replay(log: &std::path::Path) -> Vec<Record> {
    let Ok(text) = std::fs::read_to_string(log) else {
        return Vec::new();
    };
    let mut open: Vec<Record> = Vec::new();
    for raw in text.lines() {
        let Some((opens, record)) = parse_line(raw) else {
            if !raw.is_empty() {
                warn!(target: "volume", "s3: skipping an unreadable line in {}", log.display());
            }
            continue;
        };
        if opens {
            if !open.contains(&record) {
                open.push(record);
            }
        } else {
            open.retain(|held| held != &record);
        }
    }
    open
}

/// A line as `(opens, record)`: whether it opens its record or closes it.
fn parse_line(raw: &str) -> Option<(bool, Record)> {
    let mut fields = raw.split(' ');
    let (opens, temp) = match fields.next()? {
        "+" => (true, false),
        "-" => (false, false),
        "T" => (true, true),
        "t" => (false, true),
        _ => return None,
    };
    let mut next = || -> Option<String> {
        percent_decode_str(fields.next()?)
            .decode_utf8()
            .ok()
            .map(|text| text.into_owned())
    };
    let (account, bucket, key, last) = (next()?, next()?, next()?, next()?);
    if fields.next().is_some() {
        return None;
    }
    let record = if temp {
        Record::Temp(TempObject {
            account,
            bucket,
            key,
            token: last,
        })
    } else {
        Record::Upload(UnfinishedUpload {
            account,
            bucket,
            key,
            upload_id: last,
        })
    };
    Some((opens, record))
}

/// Rewrites the log to `open`, through a sibling and a rename so a crash
/// mid-write leaves the old log rather than half of a new one.
fn compact(log: &std::path::Path, open: &[Record]) {
    let temp = log.with_extension("tmp");
    let text: String = open
        .iter()
        .map(|record| match record {
            Record::Upload(upload) => line('+', upload),
            Record::Temp(object) => temp_line('T', object),
        })
        .collect();
    let rewritten = std::fs::write(&temp, text).and_then(|()| std::fs::rename(&temp, log));
    if let Err(e) = rewritten {
        warn!(target: "volume", "s3: couldn't rewrite {}: {e}", log.display());
    }
}

#[cfg(test)]
#[path = "upload_ledger_test.rs"]
mod upload_ledger_test;
