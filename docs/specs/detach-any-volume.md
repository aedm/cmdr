# Detaching any volume, from any surface

**Status**: not started. **Cost**: about half an agent-day, in four milestones that ship independently.

## The problem

Cmdr can take a volume away in four different ways, and the decision that picks between them is a ladder of predicates
about the volume rather than a question put to the volume. Every backend that arrives adds a field to the context and an
arm to the ladder, in a file that has nothing to do with that backend.

```rust
// file_system/volume/eject/mod.rs
pub fn decide_eject_action(ctx: &EjectContext) -> Result<EjectAction, EjectDecisionError> {
    if let Some(provider) = ctx.device_provider { … }   // MTP, ADB
    if ctx.is_smb { … }                                  // SMB
    if ctx.is_ejectable { … }                            // disks, DMGs
    Err(NotEjectable { … })                              // everything else
}
```

"Everything else" currently means SFTP and WebDAV, and later S3. Two consequences today:

- **An agent cannot close a remote session at all.** `connect_to_server` opens one; nothing closes one. The MCP `eject`
  tool reaches the same backend and answers `NotEjectable` for an SFTP place. The nearest tool by name,
  `remove_manual_server`, forgets the server, its places, and its pins, which is a different act.
- **The refusal is prose.** Only `EjectError::UnmountRefused` attaches typed `data` (`outcome` plus `holders`) in
  `mcp/executor/eject.rs`. Every other variant arrives as one sentence, so an agent can't tell "this kind of volume
  never ejects" from "the eject broke" — against that module's own comment that a refusal is the one answer an agent can
  act on.

The frontend half of this already bit a user. The path bar's chip asked only `isVolumeEjectable`, offered an Eject on an
open SFTP place, and got the honest answer to the wrong question; the report is `ERR-P7F5Q`. That is fixed
(`detachControlFor` decides word and action together in `navigation/detach-control.ts`), which is exactly why the
backend gap is now the visible one: the UI knows a server disconnects, and the backend still doesn't.

## Why the one-line branch is not the fix

Adding a `RemoteDisconnect` arm to the ladder would make SFTP and WebDAV work in about two hours. It would also be the
fourth ad-hoc arm, leave the agent's answer prose, and leave S3 to be a fifth. The point of this spec is that the
ladder itself is the defect.

## The shape

**Generalize the registry that already exists.** `device_volumes.rs` holds a provider registry whose members answer for
their own volumes:

```rust
pub(crate) trait DeviceVolumeProvider: Send + Sync + 'static {
    fn id(&self) -> &'static str;
    fn owns_volume_id<'a>(&'a self, volume_id: &'a str) -> ProviderFuture<'a, bool>;
    fn eject<'a>(&'a self, volume_id: &'a str) -> ProviderFuture<'a, Result<(), String>>;
    …
}
```

MTP and ADB register with it, and `owns_volume_id` is deliberately a live-state question rather than an id-shape guess.
That is already the design this spec wants; it is just scoped to devices. Widen it to "a backend that can take its own
volume away", have SFTP and WebDAV register adapters over the `disconnect` functions they already have
(`network/sftp_volume_wiring.rs::disconnect`, `network/webdav_volume_wiring.rs::disconnect`), and the ladder collapses
to two cases: a provider owns this volume, or it's host-level (a mount table entry, or a physical disk).

❗ **Not on the `Volume` trait.** That was the first idea and it's wrong: the trait lives in `crates/cmdr-fs`, which
carries no `tauri` (enforced by `index-crate-isolation`), while the session teardown for SFTP and WebDAV is app-side
wiring in `apps/desktop/src-tauri/src/network/`. Putting detach on the trait would drag app wiring into the backend
crates. The app-side registry is the right home, and it's the one already proven on this exact code path.

## What must NOT move into the registry

These are host-level concerns that no backend can answer for, and each has scar tissue:

- **The physical disk flight.** `diskutil eject` is per PHYSICAL DISK: one flight gates, stops, and resumes every volume
  on it, through `drive_release` and `disk_flight`, and the index must stop BEFORE the unmount (FSKit wedge, kernel
  panic risk). Stays exactly where it is.
- **The busy gate and the in-flight join**, both above the dispatch in `eject_now` / `eject`: never tear down a volume a
  write op is touching, and never start a second teardown for a volume already tearing down.
- **The already-unmounted short circuit**, which answers `Ok` for a drive whose root left the mount table.
- **`run_teardown` as the single place** a refusal is logged and holders are scanned.

## Milestones

**M1 — Type the answers** (~30 min, independently useful). Every `EjectError` variant carries its `outcome` tag in the
MCP `data` object, the way `unmountRefused` already does, and success returns which teardown ran rather than a sentence.
No behavior change, no new capability. Worth doing even if M2–M4 never happen, because it closes a gap against a rule
the file already states.

**M2 — One detach registry** (~3 h). Widen `DeviceVolumeProvider` into the detach-capable registry described above
(`eject()` becomes `detach()`; the doc comment stops saying "device"). `decide_eject_action` becomes: ask the registry,
else host-level. Its truth table shrinks and its tests come with it. Pure refactor: MTP and ADB keep behaving exactly as
they do, and the pure decision stays pure.

**M3 — SFTP and WebDAV register** (~1 h). Adapters over the existing `disconnect` functions, plus the registry removal
that retires the backend (`Volume::retirement`, so its watcher and reconnect loop stand down). The UI already routes
here through `runDetach`; MCP `eject` inherits it for free, which is the whole point of not adding a second verb.

**M4 — Prove the next backend is free** (~30 min). A test that registers a fake provider and detaches through it with no
edit inside `eject/`, plus the note in `volume/DETAILS.md` telling an S3 author where to register. This is the milestone
that says whether the refactor actually bought anything.

## Tests

- The pure decision's truth table, per volume shape, as it already is — smaller after M2.
- One wiring test per registering backend (`sftp_volume_wiring_test.rs` and its WebDAV twin already have the
  disconnect half; they gain the route through the registry).
- The fake-provider test from M4.
- MCP: a refusal's `data.outcome` per variant, which is M1's regression net.

## Open decisions

1. **Keep the MCP tool named `eject`?** Recommended yes. The frontend settled on "detach" internally (`DetachButton`,
   `detachControlFor`, `runDetach`) with per-kind words for humans, but `eject` is a shipped tool name and renaming it
   breaks anyone's scripts. Widen its description instead.
2. **Does detaching a remote place also forget the server?** No. Disconnect and forget are separate acts, and
   `remove_manual_server` is the second one. Recorded here so it isn't re-litigated.
3. **Should an agent be able to drop a live connection at all?** This spec assumes yes, on the grounds that Cmdr already
   lets an agent open one and the asymmetry is the bug. If the answer is no, M1 alone is the whole spec.
4. **S3, when it lands**: is detaching it a session teardown or a "remove this from the list"? Defer until the backend
   exists; M4's fake provider is the shape either way.

## Risk

The one real widening is that an automated caller could drop a live connection. It is bounded by the busy gate (a
volume a transfer is touching refuses), it matches what the button already does, and nothing here touches the disk
flight, which is the part with the panic risk.
