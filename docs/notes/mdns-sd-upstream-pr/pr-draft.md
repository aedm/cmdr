# Upstream PR draft (unsent)

For <https://github.com/keepsimple1/mdns-sd>, against `v0.21.4`. The patch beside this file is
`0001-fix-record-an-interface-whose-multicast-join-failed.patch`. Nothing has been pushed, forked, or opened; this waits
for David's go-ahead. Background and the local fork: `docs/notes/mdns-sd-multicast-join-retry-loop.md`.

**Existing issues and PRs**: searched issues (open and closed) and the last 30 PRs on 2026-09-22. Nothing covers this.
The closest matches are old Windows join failures (#47, #78, #52), which are about a join failing at startup rather than
about the retry that follows, and #459, which is about address selection for announcements. So the PR opens without an
issue reference.

---

## Title

fix: record an interface whose multicast join failed, ending an endless retry

## Body

`ServiceDaemon` retries a failed multicast join every `ip_check_interval` for as long as the interface exists, logging
each attempt. On a machine with a VM bridge it never stops.

### Symptom

```
add_interface: socket_config bridge102: PKT join multicast group on addr 192.168.64.1: Address already in use. Skipped.
```

From one production run on macOS 26 with 0.20.3, over 39 hours of retained logs:

- 9,322 retries for `bridge102` alone, at a steady five-second cadence, for the entire window.
- 87,600 of the 87,667 join attempts in that window were on these bridge interfaces. Every other interface joined once.
- 29.4 MB of the 143 MB of retained log volume was this one line.

Discovery itself kept working. The cost is log volume, the syscall churn of retrying on every tick, and everything else
in the log being buried.

### Root cause

`join_multicast_group` joins IPv4 by interface address (`join_multicast_v4(&GROUP_ADDR_V4, ip)`) rather than by index,
so two interfaces sharing one IPv4 address race for a single membership and the second `IP_ADD_MEMBERSHIP` returns
`EADDRINUSE`. macOS's Virtualization framework produces exactly that pair for every VM bridge, a `bridge1NN` alongside a
`vmenetN` with the same address, so any machine running Docker Desktop, OrbStack, or UTM has one.

`add_interface`'s `Entry::Vacant` arm then returned before inserting, so the interface never reached `my_intfs`:

```rust
Entry::Vacant(entry) => {
    if let Err(e) = join_multicast_group(&sock.pktinfo, intf) {
        debug!("add_interface: socket_config {}: {e}. Skipped.", &intf.name);
        return;
    }
    // ...
}
```

`check_ip_changes` runs every five seconds and calls `apply_intf_selections`, which calls `add_interface`. With the
interface missing from `my_intfs`, every tick treats it as new, retries the join, fails identically, logs, and returns.
The address conflict lasts as long as both interfaces do, so the loop has no exit.

### The fix

Record the interface even when the join fails, and drop the early return. This is what the `Entry::Occupied` arm a few
lines above already does when an interface gains an address: it logs the failure and inserts the address anyway. The
vacant arm taking the opposite policy looks like an oversight rather than a decision.

Two further reasons for this shape over tracking failed interfaces in a separate set:

- Joining the group is about receiving. Sending announcements and browse queries on the interface works either way, and
  an interface kept out of `my_intfs` gets neither, plus no `addr_auto` address. For the `EADDRINUSE` case specifically
  the socket already holds a membership for that address through the sibling interface, so reception is unaffected too.
- A set of failed interfaces needs a retry policy and an invalidation rule. The map already gives one: when the
  interface's address changes, `check_ip_changes` drops the stale address and the next `add_interface` takes the
  occupied arm, whose `!my_intf.addrs.contains(&intf.addr)` branch attempts the join again.

That recovery does not cover an interface that keeps its address while the _conflicting_ interface loses it: the other
interface's teardown calls `leave_multicast_v4` for the shared address, and nothing re-joins until this interface's own
address changes. That gap belongs to joining by address rather than by index, and closing it is a larger change than
this one. Happy to look at it separately if you want it.

### Test

`test_failed_multicast_join_records_interface` builds a `Zeroconf` the way the neighboring tests do and calls
`add_interface` with a synthetic interface on `192.0.2.1`. TEST-NET-1 is never assigned to a host interface, so the
kernel rejects the join the same way it rejects a conflicting address, and the test does not have to arrange two
interfaces or touch host configuration. It asserts the join really does fail before asserting anything else, so it
cannot pass vacuously on a platform that would allow it, then asserts the interface is present in `my_intfs`.

The test fails on `v0.21.4` with "an interface whose multicast join failed must still be recorded" and passes with the
change.
