package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// fakeCheckRunner stands in for `scripts/check`: it logs every invocation, rewrites
// the notices when asked for them, and fails the budget checks while the file named
// by FAKE_BUDGET_FAIL exists.
const fakeCheckRunner = `#!/bin/sh
echo "$*" >> "$FAKE_RUNNER_LOG"
case "$*" in
  *desktop-third-party-notices*) printf 'regenerated\n' > THIRD-PARTY-NOTICES.md ;;
  *file-length*) if [ -f "$FAKE_BUDGET_FAIL" ]; then echo "file-length FAILED: a.go is over budget"; exit 1; fi ;;
esac
exit 0
`

type fakeRunner struct {
	log, budgetFail string
}

// useFakeCheckRunner points the hook at fakeCheckRunner for this test.
func useFakeCheckRunner(t *testing.T) fakeRunner {
	t.Helper()
	dir := t.TempDir()
	script := filepath.Join(dir, "check")
	if err := os.WriteFile(script, []byte(fakeCheckRunner), 0o755); err != nil {
		t.Fatal(err)
	}
	f := fakeRunner{log: filepath.Join(dir, "log"), budgetFail: filepath.Join(dir, "budget-fail")}
	t.Setenv(checkRunnerEnv, script)
	t.Setenv("FAKE_RUNNER_LOG", f.log)
	t.Setenv("FAKE_BUDGET_FAIL", f.budgetFail)
	return f
}

func (f fakeRunner) calls(t *testing.T) string {
	t.Helper()
	content, err := os.ReadFile(f.log)
	if os.IsNotExist(err) {
		return ""
	}
	if err != nil {
		t.Fatal(err)
	}
	return string(content)
}

// pushedBase commits the notices and a lockfile and pushes them without the hook,
// so each test starts from a remote that already has both.
func pushedBase(r *testRepo) {
	r.write(noticesFile, "old\n")
	r.write("Cargo.lock", "v1\n")
	r.git("add", ".")
	r.git("commit", "-q", "-m", "base")
	r.git("push", "-q", "--no-verify", "origin", "main")
}

func TestPrePushRegeneratesTheNoticesWhenALockfileMoved(t *testing.T) {
	f := useFakeCheckRunner(t)
	r := newTestRepo(t)
	pushedBase(r)
	r.write("Cargo.lock", "v2\n")
	r.git("commit", "-q", "-am", "bump a crate")
	before := r.commitCount()

	out, err := r.tryGit("push", "origin", "main")
	if err == nil {
		t.Fatalf("the push went through with stale notices:\n%s", out)
	}
	if !strings.Contains(out, "Push again") {
		t.Errorf("the stopped push doesn't say what to do next:\n%s", out)
	}
	if got := r.commitCount(); got != before+1 {
		t.Fatalf("commits after the stopped push: got %d, want %d", got, before+1)
	}
	assertEqual(t, "notices commit subject", strings.TrimSpace(r.git("log", "-1", "--format=%s")), noticesCommitSubject)
	assertEqual(t, "committed notices", r.committed("HEAD", noticesFile), "regenerated\n")
	assertEqual(t, "status", r.status(), "")

	r.git("push", "-q", "origin", "main")
	assertEqual(t, "pushed notices", r.committed("remote:main", noticesFile), "regenerated\n")
	if !strings.Contains(f.calls(t), "--check desktop-third-party-notices") {
		t.Errorf("the hook never asked the runner for the notices:\n%s", f.calls(t))
	}
}

func TestPrePushLeavesTheNoticesAloneWithoutALockfileChange(t *testing.T) {
	f := useFakeCheckRunner(t)
	r := newTestRepo(t)
	pushedBase(r)
	r.write("other.txt", "x\n")
	r.git("add", ".")
	r.git("commit", "-q", "-m", "unrelated")

	r.git("push", "-q", "origin", "main")

	if strings.Contains(f.calls(t), "third-party-notices") {
		t.Errorf("the hook regenerated the notices for a push that moved no lockfile:\n%s", f.calls(t))
	}
	assertEqual(t, "pushed notices", r.committed("remote:main", noticesFile), "old\n")
}

func TestPrePushStopsAPushThatBreaksABudget(t *testing.T) {
	f := useFakeCheckRunner(t)
	r := newTestRepo(t)
	pushedBase(r)
	r.write("other.txt", "x\n")
	r.git("add", ".")
	r.git("commit", "-q", "-m", "grow a file")
	before := r.commitCount()
	if err := os.WriteFile(f.budgetFail, nil, 0o644); err != nil {
		t.Fatal(err)
	}

	out, err := r.tryGit("push", "origin", "main")
	if err == nil {
		t.Fatalf("the push went through over budget:\n%s", out)
	}
	if !strings.Contains(out, "over budget") {
		t.Errorf("the stopped push doesn't show the check's output:\n%s", out)
	}
	if r.commitCount() != before {
		t.Errorf("the hook committed for a budget failure")
	}

	if err := os.Remove(f.budgetFail); err != nil {
		t.Fatal(err)
	}
	r.git("push", "-q", "origin", "main")
	if !strings.Contains(f.calls(t), "--ci --check file-length --check claude-md-length") {
		t.Errorf("the hook didn't ask CI's question:\n%s", f.calls(t))
	}
}

// The budget scanners read files on disk, so uncommitted or untracked work would
// stand in for the pushed commit. The hook skips them then, and CI stays the gate.
func TestPrePushSkipsTheBudgetsOnADirtyTree(t *testing.T) {
	f := useFakeCheckRunner(t)
	r := newTestRepo(t)
	pushedBase(r)
	r.write("other.txt", "x\n")
	r.git("add", ".")
	r.git("commit", "-q", "-m", "grow a file")
	r.write("wip.txt", "untracked\n")
	if err := os.WriteFile(f.budgetFail, nil, 0o644); err != nil {
		t.Fatal(err)
	}

	r.git("push", "-q", "origin", "main")

	if strings.Contains(f.calls(t), "file-length") {
		t.Errorf("the hook ran the budgets over uncommitted work:\n%s", f.calls(t))
	}
}
