# Path module details

Depth and rationale. `CLAUDE.md` holds the must-knows that prevent silent breakage; this file holds the why.

## Why the `CanonicalPath` brand exists

`FilePane.svelte`'s `currentPath` is sometimes the literal string `"~"`: it's the default tab path and the home-shortcut
target, and the backend re-expands it on every IPC. Pressing Enter on the `..` row from `~` used to land at `/` because
`"~".lastIndexOf('/')` is `-1` and the fallback was `/`. Backspace had the same bug, and so did the `currentFolderName`
derivation (`"~".split('/').pop() === "~"`). Branding makes those derivations impossible without first canonicalizing.

## Why the brand is local, not viral

Only the few path-arithmetic operations (`parentOf`, `basenameOf`) require the brand; every other path-typed variable
stays `string`. `CanonicalPath` is assignable to `string`, so branded values flow outward freely; `string` is not
assignable to `CanonicalPath`, so the conversion has to be explicit. No `CanonicalPath | string` unions anywhere. This
keeps the brand from spreading into every signature while still gating the one operation that's actually unsafe.

## Virtual-volume URLs use slash arithmetic too

`mtp://device/storage/Music` becomes `mtp://device/storage` via the same `lastIndexOf('/')`. The brand exists to make
this safe to assume: anything that survived `toCanonical` is known to have a slash where the parent boundary lives.

## Canonical is not OS-resolvable

`CanonicalPath` answers "is slash arithmetic safe here", and that is all. `mtp://dev/65537/Music` passes, because the
parent boundary is where a `lastIndexOf('/')` says it is; nothing outside Cmdr can open it. The two questions look like
one and are not, so the outbound guard is a separate export rather than a stricter brand: paths flow through the app as
plain strings and only a handful of call sites hand one to an OS API.

## Volume membership is a component match

`isPathOnVolume(path, volumePath)` lives here rather than beside its first caller because three unrelated areas ask the
same question: pane navigation drops a path that doesn't belong to the volume it's pinned to, reveal-in-pane picks the
pane that can actually show a file, and the transfer dialog derives its volume-relative destination.

Two shapes make the naive `path.startsWith(volumePath)` wrong, and both reach real users:

- **A sibling root sharing a prefix.** `/Volumes/naspi` would claim `/Volumes/naspi-backup`, and a remote volume rooted
  at `/srv/data` would claim `/srv/data-1`. Appending the separator before comparing is what makes it a component match.
  `cmdr-fs`'s `RemoteRoot::to_remote_path` defends the same boundary on the Rust side, for the same reason.
- **A root that already ends in a slash.** A remote volume is rooted at `<prefix><server-side root>`, so one rooted at
  `/` spells itself `sftp://ada@nas.local:22/`. That is the DEFAULT for SFTP, WebDAV, and ADB, not an oddity.

The second shape is why `toVolumeRelativePath` trims the root before slicing. It used to slice by raw length, which ate
the separator and prefilled the copy destination with `home/ada/…`; `validateDirectoryPath` rejected it as relative and
`handleConfirm` refused to dispatch, so Copy silently did nothing on every SFTP server rooted at `/`. A volume rooted at
a subfolder carries no trailing slash and worked fine, which made it look intermittent.

`isPlainFilesystemPath` is that guard. It answers false for every virtual-volume URL and for `~`-rooted and relative
paths, since none of them resolves without a base the OS API lacks. Its live caller is the search-results clipboard
refusal (`file-explorer/pane/clipboard-operations.ts::snapshotClipboardRefusal`), where a snapshot row can name a file
on an MTP storage or an ADB device: `NSURL::fileURLWithPath` reads an unknown scheme as a RELATIVE path and returns a
file URL under the process working directory, so a missing check ships a mangled path rather than a refusal.
