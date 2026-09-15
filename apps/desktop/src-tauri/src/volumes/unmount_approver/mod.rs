//! Cmdr's answer to DiskArbitration's unmount approval: the drive is let go before any DA-mediated
//! unmount, whoever started it (Finder, `diskutil`, `hdiutil`, another app, or Cmdr itself).
//!
//! One `DASession` on its own serial queue at user-initiated QoS, so Cmdr's own indexing load can't
//! stall an ask. `ask.rs` holds the deadline and answer rules, `records.rs` what's remembered
//! between callbacks, `callbacks.rs` what each callback does over its seams. The model and the why:
//! `volumes/DETAILS.md` § "The unmount approver".

mod ask;
mod callbacks;
mod private_symbols;
mod records;

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::ptr::NonNull;
use std::sync::{Arc, OnceLock};

use cmdr_index::RemovableStop;
use dispatch2::{DispatchQoS, DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2_core_foundation::{CFArray, CFRetained};
use objc2_disk_arbitration::{
    DADisk, DADissenter, DARegisterDiskAppearedCallback, DARegisterDiskDescriptionChangedCallback,
    DARegisterDiskDisappearedCallback, DARegisterDiskUnmountApprovalCallback, DASession, DAUnregisterCallback,
    kDADiskDescriptionWatchVolumePath, kDAReturnBusy,
};

use crate::file_system::volume::drive_release::{self, DriveRelease};
use crate::volumes::disk_units;
use ask::Answer;
use callbacks::Approver;
pub(crate) use callbacks::{AskedDisk, Host};

/// Why the approver couldn't install. The app then keeps the `NSWorkspace` will-unmount observer,
/// which can't hold an unmount but still lets go of what it can.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InstallFailure {
    /// `DASessionCreate` answered NULL.
    NoSession,
}

/// What every callback reads through its context pointer.
struct Callbacks {
    approver: Arc<Approver>,
    /// The approver's own session, for the group lookup an ask makes.
    session: CFRetained<DASession>,
}

// SAFETY: a `DASession` is a CoreFoundation object whose only use here is from the one serial queue
// it's scheduled on, plus `DADiskCreateFromBSDName` / `DADiskCopyDescription`, which are MIG calls
// to `diskarbitrationd` and carry no per-thread state. `Approver`'s own state is behind a mutex.
unsafe impl Send for Callbacks {}
// SAFETY: as above.
unsafe impl Sync for Callbacks {}

/// An installed approver. Dropping it takes the session off its queue, unregisters every callback,
/// waits for a callback already running, and frees the context.
pub(crate) struct Approval {
    callbacks: Arc<Callbacks>,
    /// The same `Arc` as a raw pointer, which is what DiskArbitration hands back to each callback.
    context: *mut c_void,
    queue: DispatchRetained<DispatchQueue>,
    idle_registered: bool,
}

// SAFETY: every field is `Send`-safe (see `Callbacks`), and `context` is only ever dereferenced as
// the `Callbacks` the same struct keeps alive.
unsafe impl Send for Approval {}
// SAFETY: as above.
unsafe impl Sync for Approval {}

impl Drop for Approval {
    fn drop(&mut self) {
        let session = &self.callbacks.session;
        // SAFETY: the session is live, and each callback was registered with exactly this function
        // pointer and context. Unscheduling first stops delivery; the `exec_sync` below then waits
        // out a callback already running, since the queue is serial.
        unsafe {
            session.set_dispatch_queue(None);
            DAUnregisterCallback(
                session,
                function_pointer(unmount_approval as *const c_void),
                self.context,
            );
            DAUnregisterCallback(session, function_pointer(disk_appeared as *const c_void), self.context);
            DAUnregisterCallback(
                session,
                function_pointer(disk_disappeared as *const c_void),
                self.context,
            );
            DAUnregisterCallback(
                session,
                function_pointer(description_changed as *const c_void),
                self.context,
            );
            if self.idle_registered {
                DAUnregisterCallback(session, function_pointer(da_idle as *const c_void), self.context);
            }
        }
        self.queue.exec_sync(|| {});
        // SAFETY: `context` came from `Arc::into_raw` of a `Callbacks` in `install`, and nothing has
        // consumed it since; no callback can run any more.
        drop(unsafe { Arc::from_raw(self.context.cast::<Callbacks>()) });
    }
}

/// A callback function as the pointer `DAUnregisterCallback` takes.
fn function_pointer(callback: *const c_void) -> NonNull<c_void> {
    NonNull::new(callback.cast_mut()).expect("a function pointer is never null")
}

/// Install an approver over `gate` and `host`.
///
/// The session gets its queue LAST, so no callback can run against a half-registered session.
pub(crate) fn install(gate: DriveRelease, host: Arc<dyn Host>) -> Result<Approval, InstallFailure> {
    // SAFETY: `DASessionCreate` with the default allocator answers under the Create rule, which
    // `CFRetained` balances; NULL means the daemon couldn't be reached.
    let session = unsafe { DASession::new(None) }.ok_or(InstallFailure::NoSession)?;
    let attributes = DispatchQueueAttr::with_qos_class(DispatchQueueAttr::SERIAL, DispatchQoS::UserInitiated, 0);
    let queue = DispatchQueue::new("com.getcmdr.unmount-approver", Some(&attributes));

    let callbacks = Arc::new(Callbacks {
        approver: Approver::new(gate, host),
        session: session.clone(),
    });
    let context = Arc::into_raw(Arc::clone(&callbacks)).cast::<c_void>().cast_mut();

    // SAFETY: the session is live, each callback matches the signature its registration expects,
    // and `context` stays valid until `Approval` unregisters them all in its `Drop`. A NULL match
    // asks about every disk; the ask itself filters.
    unsafe {
        DARegisterDiskUnmountApprovalCallback(&session, None, Some(unmount_approval), context);
        DARegisterDiskAppearedCallback(&session, None, Some(disk_appeared), context);
        DARegisterDiskDisappearedCallback(&session, None, Some(disk_disappeared), context);
        DARegisterDiskDescriptionChangedCallback(
            &session,
            None,
            Some(watch_volume_path()),
            Some(description_changed),
            context,
        );
    }
    // Registering a non-approval callback kind is also what keeps the session recoverable: a
    // session DA timed out is skipped for approvals until it copies its callback queue again.
    let idle_registered = match private_symbols::register_idle_callback() {
        Some(register) => {
            // SAFETY: the session is live and `context` outlives the registration, as above.
            unsafe { register(&session, da_idle, context) };
            true
        }
        None => {
            log::warn!(
                target: "unmount_approver",
                "This macOS doesn't export DARegisterIdleCallback, so a refused unmount can't hand an index back; drives whose eject is refused stay unindexed until the next start"
            );
            false
        }
    };

    // SAFETY: the session and queue are live, and every callback is registered by now.
    unsafe { session.set_dispatch_queue(Some(&queue)) };
    log::info!(target: "unmount_approver", "The unmount approver is answering DiskArbitration (idle callback: {idle_registered})");
    Ok(Approval {
        callbacks,
        context,
        queue,
        idle_registered,
    })
}

/// The description keys the changed-callback watches: a volume's mount path, which clears when it
/// unmounts.
fn watch_volume_path() -> &'static CFArray {
    // SAFETY: the constant is an `extern "C"` `&'static CFArray` DiskArbitration global.
    unsafe { kDADiskDescriptionWatchVolumePath }
}

/// The app's one approver, kept alive for the process.
static APPROVER: OnceLock<Approval> = OnceLock::new();

/// Install the app's approver, or fall back to the will-unmount observer.
///
/// `install` and `fall_back` are parameters so a test can fail the install without DiskArbitration.
pub(crate) fn install_or_fall_back<T>(
    install: impl FnOnce() -> Result<T, InstallFailure>,
    fall_back: impl FnOnce(),
) -> Option<T> {
    match install() {
        Ok(installed) => Some(installed),
        Err(failure) => {
            crate::log_error!(
                target: "unmount_approver",
                "Couldn't install the unmount approver ({failure:?}), so Cmdr keeps the racy will-unmount handler: an unmount no longer waits for the drive's index to let go"
            );
            fall_back();
            None
        }
    }
}

/// Install the approver for the running app: one session over the app's gate, the real index, and
/// the real mount table.
pub(crate) fn install_for_app() {
    let Some(approval) = install_or_fall_back(
        || install(drive_release::gate().clone(), Arc::new(AppHost)),
        crate::volumes::watcher::install_will_unmount_observer,
    ) else {
        return;
    };
    if APPROVER.set(approval).is_err() {
        log::warn!(target: "unmount_approver", "The unmount approver was already installed; the second session is dropped");
    }
}

/// The app's answers: the volume registry, the index, the write-operation status cache, and the
/// eject flights.
struct AppHost;

impl Host for AppHost {
    fn acts_on(&self, _bsd_name: &str) -> bool {
        true
    }

    fn volume_at_active_root(&self, path: &Path) -> Option<String> {
        let (volume_id, volume) = crate::file_system::volume::manager::get_volume_manager().find_by_root(path)?;
        (volume.root() == path).then_some(volume_id)
    }

    fn is_mounted_at(&self, bsd_name: &str, path: &Path) -> bool {
        disk_units::is_volume_mounted_at(bsd_name, path)
    }

    fn stop(&self, volume_id: &str) -> RemovableStop {
        // The wait outlives the chain's budget on purpose: a stop the ask stopped waiting for still
        // answers, and its continuation records the release for the resume.
        crate::index_host::index()
            .stop_removable_volume(volume_id, crate::file_system::volume::eject::INDEX_STOP_DEADLINE)
    }

    fn is_indexing(&self, volume_id: &str) -> bool {
        crate::index_host::index().volume_kind(volume_id) == Some(cmdr_index::IndexVolumeKind::LocalExternal)
    }

    fn busy_volume_ids(&self) -> Vec<String> {
        crate::file_system::busy_volume_ids()
    }

    fn is_ejecting(&self, volume_id: &str) -> bool {
        crate::file_system::volume::eject::is_ejecting(volume_id)
    }
}

/// Run `body` against the callbacks behind `context`, answering `None` when it panicked: ❌ an
/// unwind must never cross back into DiskArbitration.
fn in_callback<T>(context: *mut c_void, body: impl FnOnce(&Callbacks) -> T) -> Option<T> {
    // SAFETY: every registration passed the `Arc<Callbacks>` pointer `install` made, and `Approval`
    // unregisters every callback before dropping that `Arc`.
    let callbacks = unsafe { context.cast::<Callbacks>().as_ref() }?;
    catch_unwind(AssertUnwindSafe(|| body(callbacks)))
        .inspect_err(|_| {
            crate::log_error!(target: "unmount_approver", "A DiskArbitration callback panicked");
        })
        .ok()
}

/// DiskArbitration is about to unmount a volume. A panic approves: the approver must never be the
/// reason a drive can't be ejected.
unsafe extern "C-unwind" fn unmount_approval(disk: NonNull<DADisk>, context: *mut c_void) -> *const DADissenter {
    let answer = in_callback(context, |callbacks| {
        // SAFETY: DiskArbitration hands a live disk object for the callback's duration.
        let disk = unsafe { disk.as_ref() };
        let Some(asked) = asked_disk(disk) else {
            return Answer::Approve;
        };
        callbacks.approver.on_unmount_ask(&asked, |whole_unit| {
            disk_units::mounted_volumes_on(&callbacks.session, &[whole_unit])
        })
    })
    .unwrap_or(Answer::Approve);
    match answer {
        Answer::Approve => std::ptr::null(),
        // SAFETY: `DADissenterCreate` answers under the Create rule, and the framework releases the
        // dissenter a callback returns, so the reference is handed over rather than dropped here.
        Answer::Dissent => CFRetained::into_raw(unsafe { DADissenter::new(None, kDAReturnBusy, None) }).as_ptr(),
    }
}

unsafe extern "C-unwind" fn disk_appeared(disk: NonNull<DADisk>, context: *mut c_void) {
    in_callback(context, |callbacks| {
        // SAFETY: DiskArbitration hands a live disk object for the callback's duration.
        if let Some(whole_unit) = whole_disk_unit(unsafe { disk.as_ref() }) {
            callbacks.approver.on_appeared(whole_unit);
        }
    });
}

unsafe extern "C-unwind" fn disk_disappeared(disk: NonNull<DADisk>, context: *mut c_void) {
    in_callback(context, |callbacks| {
        // SAFETY: DiskArbitration hands a live disk object for the callback's duration.
        if let Some(whole_unit) = whole_disk_unit(unsafe { disk.as_ref() }) {
            callbacks.approver.on_disappeared(whole_unit);
        }
    });
}

/// A watched key changed. The one watched key is the volume path, so a description with none left
/// means that volume's unmount happened, asked or not.
unsafe extern "C-unwind" fn description_changed(disk: NonNull<DADisk>, _keys: NonNull<CFArray>, context: *mut c_void) {
    in_callback(context, |callbacks| {
        // SAFETY: DiskArbitration hands a live disk object for the callback's duration.
        let disk = unsafe { disk.as_ref() };
        let Some(description) = disk_units::description(disk) else {
            return;
        };
        if disk_units::volume_path(&description).is_none()
            && let Some(bsd_name) = disk_units::bsd_name(disk)
        {
            callbacks.approver.on_volume_path_cleared(&bsd_name);
        }
    });
}

unsafe extern "C-unwind" fn da_idle(context: *mut c_void) {
    in_callback(context, |callbacks| callbacks.approver.on_idle());
}

/// The disk a callback is about, `None` when its description names no BSD unit to group by.
fn asked_disk(disk: &DADisk) -> Option<AskedDisk> {
    let description = disk_units::description(disk)?;
    Some(AskedDisk {
        bsd_name: disk_units::bsd_name(disk)?,
        volume_uuid: disk_units::volume_uuid(&description),
        whole_unit: disk_units::whole_unit(&description)?,
        path: disk_units::volume_path(&description),
    })
}

/// The BSD unit of a disk that IS a whole disk, `None` for one of its volumes.
fn whole_disk_unit(disk: &DADisk) -> Option<u32> {
    let description = disk_units::description(disk)?;
    disk_units::is_whole(&description)
        .then(|| disk_units::whole_unit(&description))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_install_that_fails_leaves_the_will_unmount_observer_in_place() {
        let mut fell_back = false;
        let installed = install_or_fall_back(|| Err::<(), _>(InstallFailure::NoSession), || fell_back = true);
        assert_eq!(installed, None);
        assert!(fell_back, "the racy pre-unmount hook is the fallback");
    }

    #[test]
    fn an_install_that_works_leaves_the_will_unmount_observer_out() {
        let mut fell_back = false;
        let installed = install_or_fall_back(|| Ok("the approver"), || fell_back = true);
        assert_eq!(installed, Some("the approver"));
        assert!(!fell_back, "two pre-unmount hooks would stop the same index twice");
    }
}
