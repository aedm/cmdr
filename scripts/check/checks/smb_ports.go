package checks

import (
	"os"
	"strconv"

	"cmdr/scripts/check/stacklease"
)

// cmdr runs its vendored copy of smb2's `consumer` SMB stack on a DEDICATED host
// port range (11480+), disjoint from smb2's own test harness, which defaults to
// 10480+. The table and the why live on the stack's registry entry
// (`stacklease.SMB`), which pins them on every compose call the lease makes, so
// any bring-up path (start.sh, this runner, e2e-linux.sh) lands on 11480+.
//
// What's left for the runner is the READ side: its children have to dial the
// same ports, so ApplySmbPortEnv exports the table into this process's env:
//   - the Rust integration tests resolve via smb2::testing::guest_port(), which
//     reads SMB_CONSUMER_*_PORT,
//   - the macOS E2E app reads SMB_E2E_*_PORT (frontend fixture + virtual hosts).
//
// The Linux Docker E2E is unaffected: it talks to the containers over the Docker
// network on their internal :445, set explicitly in its `docker run -e` (which
// overrides anything inherited).

// ApplySmbPortEnv exports cmdr's SMB host-port range into the current process
// environment, so every child process (cargo nextest, the E2E app) dials the
// ports the stack binds. Idempotent. Sets both env families: SMB_CONSUMER_*_PORT
// (guest_port and friends) and SMB_E2E_*_PORT (the E2E fixture + virtual hosts).
func ApplySmbPortEnv() {
	for svc, port := range stacklease.SMB.HostPorts() {
		p := strconv.Itoa(port)
		_ = os.Setenv(stacklease.SMB.PortEnvName(svc), p)
		_ = os.Setenv("SMB_E2E_"+svc+"_PORT", p)
	}
}
