# MTP follow-ups

MTP is `crates/cmdr-mtp`: the session layer and the `Volume` impl live in the crate, and the app keeps the hotplug
watcher, the macOS workaround, the registrar wiring, and the tauri event payloads. Where the boundary runs and why:
`crates/cmdr-mtp/DETAILS.md`; the decision record: `apps/desktop/src-tauri/src/file_system/volume/backends/DETAILS.md` §
"Per-backend decisions". What's left is one by-hand pass on hardware.

## 1. Real-device QA of the `cmdr-mtp` extraction

- **Problem**: the extraction moved MTP's session layer behind the host seams (a manager value instead of a global, a
  typed event trait instead of `tauri::AppHandle`, registration through a hook). Every automated suite ran against the
  virtual device, and nobody has walked the result on a real phone.
- **Impact**: MTP is the backend on the flakiest hardware, and the one ordering the refactor could quietly change
  (storages register BEFORE the device's event loop starts) isn't settleable statically. A regression there shows up as
  a phone that connects but never updates, or a volume that vanishes on a session reset.
- **Solution**: David, with an Android phone over USB, in order: (1) connect and list a folder; (2) copy a file to the
  phone and one back off it; (3) delete a folder that still has children (it must refuse and delete nothing); (4)
  unplug mid-copy; (5) lock the phone's screen to trigger a session reset (the volume must stay in the switcher and come
  back on its own); (6) replug into a DIFFERENT USB port (the index must re-match by serial, not rescan); (7) toggle the
  MTP setting off and on. Throughout, watch the log for registration-before-event-loop order. File a bug per failure.
- **Size**: S, about half an hour of David's time with a phone. Blocked on David and hardware.
