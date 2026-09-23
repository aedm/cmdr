package checks

import (
	"strings"
	"testing"

	"cmdr/scripts/check/stacklease"
)

// TestSmbPinnedPortsCoverEveryVendoredService guards a re-vendor of smb2's
// consumer compose: a service it adds with no entry in `stacklease.SMB`'s port
// table would fall back to smb2's 10480+ default, back on the range smb2's own
// harness squats.
func TestSmbPinnedPortsCoverEveryVendoredService(t *testing.T) {
	root := repoRootForTest(t)
	compose := readRepoFile(t, root, smbComposeRel)
	pinned := stacklease.SMB.HostPorts()

	found := 0
	for _, m := range composeDefaultRE.FindAllStringSubmatch(compose, -1) {
		svc, ok := strings.CutPrefix(m[1], "SMB_CONSUMER_")
		if !ok {
			continue
		}
		svc, ok = strings.CutSuffix(svc, "_PORT")
		if !ok {
			continue
		}
		found++
		if _, ok := pinned[svc]; !ok {
			t.Errorf("%s binds ${%s:-%s} but stacklease.SMB pins no %s port, so that service lands on smb2's range", smbComposeRel, m[1], m[2], svc)
		}
	}
	if found == 0 {
		t.Fatalf("%s declares no ${SMB_CONSUMER_*_PORT:-…} defaults, so this test asserts nothing; did the vendored compose change shape?", smbComposeRel)
	}
}
