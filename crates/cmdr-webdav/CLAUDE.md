# cmdr-webdav

The WebDAV backend: a `Volume` over one `reqwest` client with one account's Basic credentials on it. Same shape as
`crates/cmdr-sftp`, leaner: no host keys, no auth ladder, no extensions probe. Decisions and the full tables:
`DETAILS.md`. The Docker fixture stack: `apps/desktop/test/webdav-servers/start.sh`.

## Module map

- `params.rs`: `WebdavConnectionParams` (base URL, username, root under it) and the store key `scheme://host:port`
  scoped by username.
- `errors.rs`: `WebdavConnectError` and the status-code table (`map_status`, keyed by an `Attempted` context).
- `transport.rs`: `WebdavClient`, URL building, PROPFIND, the connect probe. `propfind.rs`: the `multistatus` parser.
  `liveness.rs`: the silence watch, which tells a silent server from a slow one.
- `volume/`: `mod.rs` (the volume, `connect_webdav_volume`, `send`), `paths.rs`, `query.rs`, `streams.rs` (GET),
  `writes.rs` (staged PUT + MOVE), `mutation.rs`, `copy.rs`, `scan.rs`, `state.rs` + `reconnect.rs`, `volume_impl.rs`,
  `testing.rs` (fixtures, `testing` feature).

## Must-knows

- ❗ **`reqwest` stays in `transport.rs`, `errors.rs`, `streams.rs`, `writes.rs`, and `volume/mod.rs`'s `send`.**
  Everything else works in `Url`s, `StatusCode`s, and `PropfindEntry`s. Its features are EXACTLY a subset of the app's;
  a second configuration would enter the graph twice.
- ❌ **Never classify by message.** A status by number plus `Attempted`; a `reqwest::Error` by `is_timeout` /
  `is_connect` / `is_request`; a TLS refusal by the `io::ErrorKind::InvalidData` in its source chain.
- ❗ **Every wire-touching delegator wraps itself in `noting`.** No watcher, no session: the operations ARE the
  detector. `noting` also races each one against its client's `lost` token, so a server that goes SILENT (10 s quiet,
  then two unanswered probes: 30 s) cuts them with `DeviceDisconnected` exactly like a refused one, which flips the state
  once and starts the backoff. `DETAILS.md` § "Silent or slow".
- ❗ **Every request goes out through `WebdavClient::send` or `propfind`**, and a body read counts its chunks as
  `heard`. A path that skips both looks silent to the watch however much it hears.
- ❌ **No `read_timeout`, no `.timeout()` on the streaming PUT or GET, and never a timeout read as a lost server**: a
  long transfer or a slow listing would be cut or flicker `Disconnected`. `transport.rs` has why.
- ❌ **One unattended authentication attempt, never a loop.** A 401 on the re-probe moves to `NeedsCredentials` and
  stops. The store is only ever refreshed by an attended sign-in, never seeded.
- ❗ **Redirects are off**: a followed MOVE or COPY would resend `Destination` somewhere the user never named.
- ❗ **PUT sends `Content-Length` from `size`** onto a `.cmdr-tmp-*` sibling, MOVEd into place with `Overwrite: T`. ❌ A
  source whose byte count disagrees with `size` is never MOVEd: hyper truncates a longer body and the server stores the
  prefix happily.
- ❗ **The upload body reads one piece AHEAD, and that is load-bearing**: hyper stops polling once `Content-Length` is
  satisfied. Hence two counters: `fetched` guards the size, `handed` (clamped) drives progress. ❌ Never collapse them.
  `DETAILS.md` § "Write staging".
- ❗ **`Range` may be ignored**: a 200 to a ranged GET is skipped locally (fixture `webdav-fixture-norange`).
- ❗ **Unbounded space reads `oc:size`, ❌ never `quota-used-bytes`** (an unlimited Nextcloud answers `0`). The only
  non-standard property; `DETAILS.md` § "What a real server answers" sets the bar for a second.
- ❗ **DELETE is recursive by protocol; the trait's is not.** `delete` refuses a non-empty collection with `ENOTEMPTY`
  after a `Depth: 1` PROPFIND.
- ❌ Never `root_anchored`, never a stat per child in a scan, never `authoritative_listing` (coverage is `None`).
- Digest-only servers are a typed `AuthMethodUnsupported`, by the `WWW-Authenticate` scheme token.
