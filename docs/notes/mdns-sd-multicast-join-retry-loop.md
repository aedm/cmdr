# The `mdns-sd` multicast-join retry loop, and the fork that stops it

Why `vendor/mdns-sd` exists, what one line of it changes, and what has to happen before it can go away.

## Symptom

`ServiceDaemon` logged the same line forever, at exactly five-second intervals, on any machine running Docker Desktop,
OrbStack, or UTM:

```
add_interface: socket_config bridge102: PKT join multicast group on addr 192.168.64.1: Address already in use. Skipped.
```

Measured on one production run (macOS 26, `mdns-sd` 0.20.3, 39 hours of retained logs, 2026-09):

- 9,322 retries for `bridge102` alone, at a steady five-second cadence, for the whole 39 hours.
- 87,600 of 87,667 total join attempts in the window were on these bridge interfaces. Everything else joined once.
- 29.4 MB of the 143 MB of retained log volume was this one line.

The daemon kept working: discovery over the real interfaces was unaffected. The cost was log volume, the CPU and syscall
churn of retrying every tick, and the noise burying anything else in the log.

## Root cause

Two pieces of upstream combine into a loop with no exit.

1. `join_multicast_group` joins IPv4 by interface **address** (`join_multicast_v4(&GROUP_ADDR_V4, ip)`), not by index.
   Two interfaces that share one IPv4 address therefore race for one membership, and the second `IP_ADD_MEMBERSHIP`
   comes back `EADDRINUSE`. macOS's Virtualization framework creates exactly that shape: every `bridge1NN` is paired
   with a `vmenetN` carrying the same address, so any VM or container runtime puts one on the machine.
2. `add_interface`'s `Entry::Vacant` arm returned early when the join failed, so the interface never reached
   `self.my_intfs`.

`check_ip_changes` runs every five seconds, calls `apply_intf_selections`, which calls `add_interface`. With the
interface absent from `my_intfs`, every tick sees it as new, retries the join, fails the same way, logs, and returns.
The failure is permanent (the address conflict lasts as long as both interfaces do), so the retry is permanent too.

Verified identical in 0.21.4; only `add_interface`'s signature differs
(`fn add_interface(&mut self, intf: &Interface, interfaces: &[Interface])`).

## The fix

Record the interface even when the join fails, which is what the `Entry::Occupied` arm a few lines above already does
for an added address. The early `return` is the whole bug.

```rust
Entry::Vacant(entry) => {
    if let Err(e) = join_multicast_group(&sock.pktinfo, &intf) {
        debug!("add_interface: socket_config {}: {e}", &intf.name);
    }
    new_addr = true;
    // ... unchanged: build MyIntf, entry.insert(new_intf)
}
```

### Why this shape and not a failed-interface set

The alternative was tracking failed interfaces in a separate set and skipping them on later ticks under some retry
policy. Three things argue against it:

- **The codebase already models "known interface, failed join".** The occupied arm inserts an address whose join failed
  and only logs. A vacant interface taking the opposite policy is an inconsistency, not a decision.
- **Joining is about receiving, not sending.** An interface left out of `my_intfs` gets no announcements, no browse
  queries, and no `addr_auto` address, which costs more than the membership does. For the `EADDRINUSE` case it costs it
  for nothing: the socket already holds a membership for that address through the sibling interface, so packets still
  arrive.
- **A set needs an invalidation story that the map gives for free.** Recovery has to happen when the conflict resolves,
  and a set of names has nothing to hang that on but a timer.

### What recovery looks like, and where it stops

When the interface's own address changes, `check_ip_changes` removes the stale address and the next `add_interface`
takes the occupied arm, whose `!my_intf.addrs.contains(&intf.addr)` branch attempts the join again. That covers the
common resolution: the bridge gets a different address, or goes away and comes back.

It does not cover the case where the interface keeps its address and the _conflicting_ interface loses it. The other
interface's teardown calls `leave_multicast_v4` for the shared address, and nothing re-joins until this interface's own
address changes. That gap is upstream's join-by-address design (two interfaces, one membership) rather than anything
this fix introduces, and closing it means joining by index, which is a much larger change.

## The test

`test_failed_multicast_join_records_interface` in `vendor/mdns-sd/src/service_daemon.rs` builds a `Zeroconf` the way the
neighboring tests do, then calls `add_interface` with a synthetic interface on `192.0.2.1` (TEST-NET-1, never assigned
to a host interface, so the kernel rejects the join the same way a conflicting address does). It asserts the join really
fails first, so the test can't pass vacuously on a platform that allows it, then asserts the interface is in `my_intfs`.

Red before the fix ("an interface whose multicast join failed must still be recorded"), green after. The rest of the
crate's unit tests are unaffected: `test_custom_port_isolation` and `test_legacy_unicast_response` fail on David's
machine both before and after (they need real mDNS reception, which local network permissions block for a terminal
binary).

## How the fork is carried

`vendor/mdns-sd/` is the published 0.20.3 source, byte-identical to the crates.io tarball except for the fix, its test,
and one `[workspace]` table added to `Cargo.toml`. The root `Cargo.toml` swaps it in with `[patch.crates-io]`.

- **It is not a workspace member.** Membership would put upstream's network test suite into every `--workspace` test
  lane and every house scanner onto code whose value is that it still matches upstream. It still compiles on every build
  of `cmdr`, because that is where the dependency is.
- **`[workspace]` in its manifest is load-bearing.** `exclude` in the root manifest is not enough on its own: a worktree
  lives under `.claude/worktrees/`, so cargo walks past the excluded path and finds the main clone's workspace instead.
- **Repo-wide tooling treats `vendor/` as out of jurisdiction**: `file-length` skips it (`fileLengthSkipDirs`), oxfmt
  and Prettier ignore it, and the doc graph already did.
- **Running its tests** means `cargo test --lib` inside `vendor/mdns-sd`; no lane runs them.

## Dropping the fork

Upstream is <https://github.com/keepsimple1/mdns-sd>. The PR text is in `docs/notes/mdns-sd-upstream-pr/pr-draft.md` and
the patch against `v0.21.4` sits beside it as
`docs/notes/mdns-sd-upstream-pr/0001-fix-record-an-interface-whose-multicast-join-failed.patch`. Both are unsent as of
2026-09-22: nothing is forked, pushed, or opened.

Once the fix is in a release, delete `vendor/mdns-sd`, delete the `[patch.crates-io]` block and the `exclude` entry from
the root `Cargo.toml`, bump the dependency in `apps/desktop/src-tauri/Cargo.toml` to that version, and regenerate
`THIRD-PARTY-NOTICES.md`. The `vendor` entries in `.oxfmtrc.json`, `.prettierignore`, and `fileLengthSkipDirs` can stay;
they cost nothing and the next vendored crate wants them.
