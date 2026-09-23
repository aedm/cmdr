# SFTP follow-ups

The backend and its IPC surface are `crates/cmdr-sftp` (canonical account: `crates/cmdr-sftp/DETAILS.md`), and the
frontend it shares with WebDAV is `apps/desktop/src/lib/servers/DETAILS.md`. Two items are open, both deferred until a
user asks.

## 1. Free space and non-UTF-8 filenames, both via vendoring the SFTP protocol crates

- **Problem**: two gaps with one fix. `get_space_info` answers `NotSupported`, so a pane never shows how full an SFTP
  server is: `statvfs@openssh.com` is unreachable from the current crate stack (no request to send it, no predicate to
  ask whether the server has it). And a filename that isn't valid UTF-8 costs the whole SESSION, not only the listing
  that hit it, so a server holding one such name is unusable.
- **Impact**: low today. No user has reported either. The non-UTF-8 failure is loud and lossless, which beats the
  alternative crate's U+FFFD substitution (a name that addresses nothing).
- **Solution**: vendor `openssh-sftp-protocol` and `ssh_format` under `crates/` as **path** dependencies (❌ not
  `git =`: `deny.toml` denies unknown git sources), then add the `statvfs` request and make `NameEntry::filename`
  byte-backed. The detail and the pinned behavior: `crates/cmdr-sftp/DETAILS.md` § "4. A filename that isn't UTF-8
  costs the SESSION" and § "The `Volume` answers, and why" (the `get_space_info` bullet; the app-side half of that
  contract is already paid).
- **Size**: L. Roughly 2,750 lines of vendored `src/` (mostly protocol tables nobody edits after the first read), plus
  a permanent maintenance obligation on two crates. **Blocked on a trigger**: a real user report of either gap.

## 2. `~/.ssh/config` host aliases as completions in the add form's address field

- **Problem**: someone who reaches a server as `ssh naspi` has to retype `ada@nas.local:2222` into the add form,
  because Cmdr never reads `~/.ssh/config`. The alias is the name they know the machine by, and it already carries the
  host, the port, the user, and often the identity file.
- **Impact**: friction for exactly the audience most likely to use SFTP. The add form works without it.
- **Solution**: a backend command that parses `~/.ssh/config` (including `Include`) and answers a list of
  `{ alias, hostname, port, user, identityFile }`, which the sheet's address field offers as completions; picking one
  fills the endpoint fields and leaves them editable, as the address parser's own answer does. ❌ Read-only, and ❗ never
  write to `~/.ssh/config` or `~/.ssh/known_hosts` (a standing rule in `apps/desktop/src-tauri/src/network/CLAUDE.md`).
  Edge cases to decide: `Match` blocks, tokens like `%h`, and an alias naming a `ProxyJump` Cmdr can't honor. **Needs a
  David decision**: offer such an alias and let the connect refuse, or hide it.
- **Size**: M, about a day for a parser handling `Host`, `HostName`, `Port`, `User`, `IdentityFile`, and `Include`,
  plus the frontend completion source.
