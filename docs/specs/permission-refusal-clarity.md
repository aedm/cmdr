# Permission refusals: name the folder, drop the injection hole

**The quest**: split out of `elevated-file-operations.md` as its M0, because neither half needs the root helper and both
are worth having on their own.

1. A user (ERR-4TEMD, v0.44.0) tried to move two `root:admin` files out of
   `/Applications/PixInsight/src/scripts/Toolbox/`. The rename was refused because the SOURCE folder takes no writes
   from their macOS user, and Cmdr told them to "check that you have write access to the destination folder". They
   finished the job with `sudo` in Terminal.
2. `apps/desktop/src-tauri/Entitlements.plist` carries `com.apple.security.cs.disable-library-validation`, which is the
   standard way to inject code into a signed app. It's an unnecessary hole today, and from the helper's M2 on it would
   be the hole that makes the whole design pointless: decision 8 of the helper spec trusts whatever runs inside Cmdr.

**What we're building**: a permission refusal that names the folder that actually refused and says whether administrator
rights could change the answer, plus a bundle nothing can inject into.

## Decisions

1. **The refusing folder is proved, never guessed.** A `rename(2)` needs write access to BOTH parents, so the errno
   alone can't say which one refused. On the refusal path only, Cmdr probes the candidate folders with `access(W_OK)`
   and names the first that refuses. No probe runs on the happy path.
2. **Nothing is named when nothing can be proved.** A refusal where every candidate folder answers "writable" (an ACL, a
   race, a read refusal rather than a write one) carries no folder, and the message stays today's generic one. ❌ We
   never name a folder on the strength of the operation's shape alone.
3. **`EACCES` and `EPERM` are different answers and get different advice.** `EACCES` is the folder's own permissions,
   where an administrator could do it; `EPERM` is macOS itself (SIP, an immutable flag, a privacy protection), where
   administrator rights change nothing. Rust folds both into `ErrorKind::PermissionDenied`, so the errno rides along as
   a typed field. This is also what the helper's decision 12 branches on in M3.
4. **The raw errno travels too**, for the technical-details block and bug reports, the way `MoveNotConfirmed` already
   carries one. The classification in decision 3 is derived from it by one constructor, so the two can't disagree.
5. **`disable-library-validation` goes, and the drop is verified on a real signed build.** Tauri's WebView runs out of
   process, so library validation shouldn't be load-bearing; the entitlement arrived in `ff0c27ee4` with no stated
   reason beyond a comment claiming the WebView needs it. If a signed build proves otherwise, the fallback is the helper
   spec's decision 9 (the XPC client and the alert move into a separate signed binary).
6. **`allow-unsigned-executable-memory` goes with it.** It's for a process that JITs, and WKWebView's JavaScript runs in
   Apple's own out-of-process `com.apple.WebKit.WebContent`. Tested in the same signed-build round, since the round trip
   was already being paid for.

## Milestones

- [x] **M1: the entitlements.** Both dropped; the plist is empty. Verified on a signed universal build (hardened runtime
      `flags=0x10000`, Developer ID, `codesign --verify --deep --strict` clean, `codesign -d --entitlements` prints an
      empty dict): the app launches, both panes list files, and Svelte renders its dialogs, so the WebView needs neither
      entitlement. Notarization proved too (2026-09-19, submission `94b2817d`): `Accepted`, stapled, and
      `spctl -a -vvv -t install` reports `source=Notarized Developer ID`.
- [x] **M2: the typed refusal.** `PermissionDenied` carries `errno`, the derived `PermissionRefusal`, and
      `refused_folder`, all through one constructor. The probe (`validation::refusing_folder`) sits inside
      `classify_io_error`, so every local-FS refusal gets it with no new call sites: the rename engines, the delete
      walker, the native copy calls, and the destination pre-flight all reach it already. Backends that word their own
      refusals (MTP, SMB) carry neither, since nothing there proved one.
- [x] **M3: the copy.** The message names the folder, and the suggestion splits on whether administrator rights would
      help. Five new `errors.write.permissionDenied.*` keys, translated into all 10 locales with Apple's own Finder
      wording mined from `Finder.app`'s `LocalizableMerged.strings` (`N30` / `NE43`) per locale. ❗ The English is a
      draft awaiting David's review.

## Notes

- Notarizing a local build turned out to be possible, and the recipe is now
  `docs/guides/apple-signing-and-notarization.md` § "Notarizing a local build". The trap it documents: a local
  `pnpm build` leaves the AI binaries ad-hoc signed, and Apple's first `Invalid` was about those, not about the
  entitlements.
- Sticky-bit and TCC refusals (open questions 3 and 4 in `elevated-file-operations.md`) become observable once the errno
  travels, so M2 may hand the helper's M1 spike free evidence.
