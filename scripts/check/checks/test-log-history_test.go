package checks

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

func writeTestLog(t *testing.T, rows string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "test-log.csv")
	header := "timestamp,check,test_id,status,duration_s,attempt\n"
	if err := os.WriteFile(path, []byte(header+rows), 0o644); err != nil {
		t.Fatalf("writing the fixture log: %v", err)
	}
	return path
}

// The question an agent asks first: has this spec gone red before, and when last?
func TestLookupTestHistoryCountsFailuresAndTheLastOne(t *testing.T) {
	path := writeTestLog(t, `2026-09-01 10:00:00,desktop-e2e-playwright,dup.spec.ts::::duplicates a file,pass,1.400,1
2026-09-10 10:00:00,desktop-e2e-playwright,dup.spec.ts::::duplicates a file,fail,15.010,1
2026-09-16 11:00:00,desktop-e2e-linux,dup.spec.ts::::duplicates a file,fail,15.020,1
2026-09-17 11:00:00,desktop-e2e-playwright,dup.spec.ts::::duplicates a file,flaky,3.200,2
2026-09-18 11:00:00,desktop-e2e-playwright,other.spec.ts::::something else,fail,1.000,1
`)

	history, err := LookupTestHistory(path, []string{"dup.spec.ts::::duplicates a file"}, time.Minute)
	if err != nil {
		t.Fatalf("LookupTestHistory: %v", err)
	}
	h := history["dup.spec.ts::::duplicates a file"]
	if h.Runs != 4 {
		t.Errorf("Runs = %d, want 4", h.Runs)
	}
	if h.Failures != 2 {
		t.Errorf("Failures = %d, want 2 (both lanes run the same spec)", h.Failures)
	}
	if h.Flakes != 1 {
		t.Errorf("Flakes = %d, want 1", h.Flakes)
	}
	if h.LastFailure != "2026-09-16" {
		t.Errorf("LastFailure = %q, want 2026-09-16", h.LastFailure)
	}
	if _, asked := history["other.spec.ts::::something else"]; asked {
		t.Error("a test nobody asked about must not be accumulated")
	}
}

// A timeout row is a failure: the runner killed the spec at its cap, which is exactly
// the outcome this whole mechanism exists to explain.
func TestLookupTestHistoryCountsTimeoutsAsFailures(t *testing.T) {
	path := writeTestLog(t, `2026-09-10 10:00:00,desktop-e2e-playwright,slow.spec.ts::::waits,timeout,15.000,1
`)
	history, err := LookupTestHistory(path, []string{"slow.spec.ts::::waits"}, time.Minute)
	if err != nil {
		t.Fatalf("LookupTestHistory: %v", err)
	}
	if h := history["slow.spec.ts::::waits"]; h.Failures != 1 || h.LastFailure != "2026-09-10" {
		t.Errorf("a timeout must count as a failure, got %+v", h)
	}
}

// The log's columns are not ours to assume: it's read by position nowhere, by name
// everywhere, so a future column can be appended without silently shifting the read.
func TestLookupTestHistoryReadsColumnsByName(t *testing.T) {
	path := filepath.Join(t.TempDir(), "test-log.csv")
	body := "check,test_id,extra,status,timestamp,duration_s,attempt\n" +
		"desktop-e2e-playwright,dup.spec.ts::::x,whatever,fail,2026-09-10 10:00:00,15.000,1\n"
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatalf("writing the fixture log: %v", err)
	}

	history, err := LookupTestHistory(path, []string{"dup.spec.ts::::x"}, time.Minute)
	if err != nil {
		t.Fatalf("LookupTestHistory: %v", err)
	}
	if h := history["dup.spec.ts::::x"]; h.Failures != 1 || h.LastFailure != "2026-09-10" {
		t.Errorf("columns should be resolved by header name, got %+v", h)
	}
}

// Instrumentation must never colour a verdict: a missing log is an error the caller
// can mention and ignore, never a partial count dressed up as the truth.
func TestAMissingLogIsAnErrorNotAnEmptyHistory(t *testing.T) {
	if _, err := LookupTestHistory(filepath.Join(t.TempDir(), "nope.csv"), []string{"a"}, time.Minute); err == nil {
		t.Error("a missing log must be reported, not silently read as 'never failed'")
	}
}

// A `test_id` contains commas and quotes; the log is RFC 4180 and must be parsed as
// such, or the rows this exists to find are the ones that get mangled.
func TestLookupTestHistoryHandlesQuotedIDs(t *testing.T) {
	id := `dup.spec.ts::Copy, move, and delete::handles "quoted" names`
	path := writeTestLog(t, "2026-09-10 10:00:00,desktop-e2e-playwright,\"dup.spec.ts::Copy, move, and delete::handles \"\"quoted\"\" names\",fail,15.000,1\n")

	history, err := LookupTestHistory(path, []string{id}, time.Minute)
	if err != nil {
		t.Fatalf("LookupTestHistory: %v", err)
	}
	if history[id].Failures != 1 {
		t.Errorf("a quoted test_id must round-trip, got %+v", history[id])
	}
}

// Asking for nothing must cost nothing: a green lane never opens the log at all.
func TestLookupTestHistoryWithNoIDsReadsNothing(t *testing.T) {
	history, err := LookupTestHistory(filepath.Join(t.TempDir(), "nope.csv"), nil, time.Minute)
	if err != nil {
		t.Errorf("no ids means no work and no error, got %v", err)
	}
	if len(history) != 0 {
		t.Errorf("expected an empty history, got %+v", history)
	}
}
