# Updater module

Custom macOS updater that syncs files *into* the existing `.app` bundle, keeping its `com.apple.macl` xattr and giving
each per-file write the fresh inode the code-signing cache needs. macOS-only (`#[cfg(target_os = "macos")]`); other
platforms use the Tauri updater plugin and the frontend calls the plugin API directly.

## File map

- `mod.rs`: the four Tauri commands (`check_for_update`, `update_write_blocker`, `download_update`, `install_update`),
  shared `UpdateState` (what the last check offered, what the last download staged), and the typed outcome and errors.
  Tests in `tests.rs`.
- `bundle_location.rs`: whether the running bundle can be written into at all (`BundleWriteBlocker`).
- `manifest.rs`: parses `latest.json`, compares versions, resolves the platform key.
- `signature.rs`: minisign signature verification (base64-wrapped, matching Tauri's format).
- `installer.rs`: tarball extraction, sync into the running bundle, privilege escalation. Its
  `running_bundle()` is crate-visible: `crate::install_location` asks the same "where is this copy installed" question
  on behalf of the Dock pin and the reveal handler.

## Must-knows

- **Sync into the bundle, never replace the `.app` directory**, because the per-file atomic rename below needs a bundle
  to sync into. ❌ NOT because replacing loses the FDA grant: it doesn't (measured; `DETAILS.md`).
- **Per-file writes use atomic rename (temp + `rename()`), not in-place `fs::copy`.** `fs::copy` keeps the same inode;
  macOS's kernel code-signing cache keys on inode and validates the new binary against the old cached code directory,
  causing `SIGKILL (Code Signature Invalid)` on launch. A new inode forces fresh validation. The admin path (`rsync -a`)
  already renames atomically.
- **Staging dir is per-instance: `<tmp>/cmdr-update-staging-{CMDR_INSTANCE_ID}`** (`installer::staging_dir`; production
  with no env var lands at `…-default`). Don't make it shared: concurrent `Cmdr` processes (main + a worktree) race on
  one path and trip `ENOTEMPTY`.
- **Only a real user's production install may check** (`skip_reason`): inside a `.app` bundle, and none of
  `crate::prod_instance::NON_PROD_ENV_VARS` set. Otherwise it spams the error reporter or inflates active installs.
  ❌ Never keep a second copy of the env-var list here. `DETAILS.md` § Who may check.
- **A read-only bundle is EROFS, not EPERM, and no amount of admin fixes it.** App Translocation (Cmdr opened from
  `~/Downloads`) and a mounted `.dmg` both put the bundle on a read-only mount, which refuses root as flatly as the
  user. `installer::install` and the frontend both gate on `bundle_location::classify` BEFORE the download, ❌ never by
  escalating: escalating buys an auth dialog the user can only cancel. The `PermissionDenied` arm is for a root-owned
  `/Applications`, a different thing. `DETAILS.md` § A bundle that can't be written.
- **The signature doesn't name a version, so the archive's own `Info.plist` does.** Its trusted comment is only
  `file:Cmdr.app.tar.gz`, and `latest.json` isn't signed, so `installer::refuse_unless_newer` refuses a staged bundle
  whose `CFBundleShortVersionString` isn't newer than the running build. ❌ Never install around it: it's what stops an
  older signed release from rolling an install back.
- **The organization's policy gates every step, and the backend decides.** `check_for_update(trigger)` answers a typed
  `UpdateCheckOutcome` (`DisableUpdates` and a refused background check return before any request); `download_update`
  takes no URL and fetches only what the last check offered; `install_update` re-asks
  `ManagedPolicy::update_to` on a fresh read, then again for the version the extracted `Info.plist` names. ❌ Never let
  the frontend name a URL or version again. `DETAILS.md` § Managed policy.
- **Manifest fetch is bounded** (`connect_timeout` 10 s, overall `timeout` 30 s); download/install paths are
  intentionally NOT timed out (they run with user attention). Don't add timeouts there.
- **Manifest URL routes through the API server** (`api.getcmdr.com/update-check/{version}?arch={arch}`, which counts
  the check, then 302s to `getcmdr.com/latest.json`).

Full details (sync order, deletion pass, minisign rationale, privilege escalation, error-chain logging,
dependencies): `DETAILS.md`.
