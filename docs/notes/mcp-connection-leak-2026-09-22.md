# The MCP server does not leak connections (2026-09-22)

**Verdict: not a leak.** A long-running prod app holding a handful of ESTABLISHED sockets on the MCP port is the
expected shape of HTTP/1.1 keep-alive against live clients, not dead sessions the server forgot to reap. The suspicion
that started this ("13 ESTABLISHED connections after 25 hours = 13 dead sessions") was wrong. Read this before reopening
the question.

All measurements: prod `/Applications/Cmdr.app` PID 69181, 26 hours uptime, macOS 27.0.0, 2026-09-22, via `lsof -nP`,
`netstat -anv -p tcp`, and a purpose-built connection holder.

## What was actually observed

At rest, the server held **three** ESTABLISHED connections, and all three belonged to one **live** `claude` process (PID
44100, started four minutes earlier). Meanwhile three _older_ Claude Code sessions (up 4 to 6 days) held **zero**
connections between them.

That split is the whole answer: connection count tracks currently-active clients, not accumulated history. A session
that has been idle for days holds nothing. Within a few minutes of unattended observation the live client dropped from
three connections to one on its own, and the server reused the freed descriptors (fds 152 and 153) for new connections,
which is direct evidence that closed sockets are released rather than retained.

## The mechanism: the client holds the socket, not the server

Nothing in the server holds a connection open.

- `apps/desktop/src-tauri/src/mcp/server.rs:387` serves with a bare `axum::serve(listener, router)`: no idle timeout, no
  keep-alive reaper, no connection cap. Each accepted connection is one hyper task that ends when the peer closes.
- `handle_mcp_get` (`server.rs:483-518`) looks like it would hold a long-lived SSE stream, and it is the obvious
  suspect, but it does not. Its body is `stream::once` (`server.rs:513`), which completes immediately, so the
  `.keep_alive(KeepAlive::new())` on `server.rs:516` never fires: axum's keep-alive only injects comment pings while the
  inner stream is `Pending`, and this one never pends. **Verified**: `curl -N http://127.0.0.1:19224/mcp` returns the
  single `event: endpoint` and exits in 0.035 s.

So the long-lived sockets are ordinary idle HTTP/1.1 keep-alive connections parked in the _client's_ pool. The server
simply does not force them shut, which is correct behavior.

## The decisive experiment: a dead client's socket is reaped instantly

Opened four keep-alive connections from throwaway `bash` processes, confirmed the server accepted them (server-side
ESTABLISHED went 1 to 5), then `kill -9`ed the holders so they could never close cleanly.

**Every server-side socket was gone at the first sample after the kill** (under one second), back to one ESTABLISHED,
with nothing left in `netstat` on the server side. Only the client-side `TIME_WAIT` entries remained, which is normal
2MSL teardown on the killed processes' side and invisible to Cmdr.

This is the expected kernel behavior and it is worth stating plainly, because it is what makes the leak hypothesis
impossible: a process exit always closes its descriptors, so a loopback peer that dies sends a FIN, and hyper drops the
task on EOF. **An ESTABLISHED socket whose peer process is gone essentially cannot persist on loopback.** Any
ESTABLISHED connection on the MCP port therefore has a live client at the other end, by construction.

## What a connection costs, and what bounds it

Scale test: 100 idle keep-alive connections held open against the live prod app.

- **File descriptors: exactly one each**, 338 to 438, and back to **exactly 338** after `kill -9`. Full recovery, no
  residue.
- **Memory: about 46 KB per connection** (RSS 173,808 KB to 178,416 KB, so roughly 4.6 MB for 100). Treat this as an
  order of magnitude, not a precise figure: the app is live and its RSS drifts on its own between samples, and mimalloc
  does not return freed arenas to the OS promptly.
- **Per connection**: one fd, one tokio task, and hyper's read/write buffers. Nothing else.

**Nothing accumulates per session.** `McpState` (`server.rs:72-78`) holds a single `session_id: RwLock<Option<String>>`
and a single `negotiated_version`, not a map. There is no `HashMap` / `DashMap` / `BTreeMap` anywhere in
`apps/desktop/src-tauri/src/mcp/*.rs`. There is no session registry to grow, no per-session subscription, and no channel
kept alive past the request.

The count is bounded by live clients times their pool size. It is not bounded by an explicit cap, so a pathological
client could still exhaust descriptors, but reaching even 338 open fds took 26 hours of heavy multi-session use against
a limit the app has already raised well past the 256 default.

## Does this reach real users?

**No, and it is not latent either.** A normal user runs at most one MCP client and most run none. David's pattern (many
concurrent agent sessions against one long-running app) is the extreme case, and it produced three connections and about
140 KB. To get somewhere uncomfortable a user would need hundreds of simultaneously-live MCP clients, which no real
workflow produces.

The one honest caveat: there is no connection cap, so a buggy or hostile local client in a tight connect loop could
climb. That is a hardening question about a localhost port that is already bearer-token gated, not a leak, and it is not
worth spending on now.

## Unrelated finding: two SMB sockets stuck in CLOSE_WAIT

While inventorying the app's 26 sockets, two have sat in `CLOSE_WAIT` on port 445 for at least 12 minutes, unchanged: fd
90 to `192.168.1.111:445` and fd 193 to `100.127.48.122:445` (a Tailscale peer).

`CLOSE_WAIT` means the peer sent a FIN and Cmdr never called `close()`. Unlike the MCP sockets, these do **not** self-
reap, so each one is a descriptor held forever. Two is harmless, but the count only moves one way across an app's
lifetime, and both peers are ones that go away (a sleeping NAS, a roaming Tailscale host), so this is worth a look in
the SMB layer's teardown path. Not investigated here; flagging only.

**Resolved in `smb2` 0.24.1 (2026-09-23).** Two `smb2` defects, not Cmdr's: a server hang-up left the socket in
`CLOSE_WAIT` until the `Connection` dropped (these two), and a dropped `Connection` never closed its socket at all until
the server hung up (78 of those to the Docker fixtures in the same prod process, from E2E mount/unmount cycles). Cmdr
needed only the version bump. The ownership rules that prevent both live in the `smb2` repo, in the client module's
agent doc, § "Socket lifetime".
