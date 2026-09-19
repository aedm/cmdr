package main

import (
	"encoding/csv"
	"os"
	"path/filepath"
	"testing"

	"cmdr/scripts/check/checks"
)

// redirectHome points the log writers at a throwaway home for one test. It
// clears $XDG_DATA_HOME too: that variable outranks HOME in `logPath`, so
// leaving it to the ambient environment would let a machine that sets it write
// real rows into the developer's actual log.
func redirectHome(t *testing.T) string {
	t.Helper()
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("XDG_DATA_HOME", "")
	return home
}

// wantLogPath spells out the location contract the logs are expected to honor,
// literally rather than through `logPath`, so a change to the resolver moves
// these tests to red instead of silently following it.
func wantLogPath(home, fileName string) string {
	return filepath.Join(home, ".local", "share", "check-runner", "cmdr", fileName)
}

func TestLogPathPutsEveryLogUnderTheSharedCheckRunnerDir(t *testing.T) {
	home := redirectHome(t)

	for _, name := range []string{csvFileName, testCSVFileName, unknownSelectorCSVFileName} {
		got, err := logPath(name)
		if err != nil {
			t.Fatalf("logPath(%q): %v", name, err)
		}
		if want := wantLogPath(home, name); got != want {
			t.Errorf("logPath(%q) = %q, want %q", name, got, want)
		}
	}
}

func TestLogPathPrefersXDGDataHome(t *testing.T) {
	redirectHome(t)
	t.Setenv("XDG_DATA_HOME", "/tmp/xdg")

	got, err := logPath(csvFileName)
	if err != nil {
		t.Fatalf("logPath: %v", err)
	}
	if want := filepath.Join("/tmp/xdg", "check-runner", "cmdr", csvFileName); got != want {
		t.Errorf("logPath = %q, want %q", got, want)
	}
}

// readTestLog returns the rows of the per-test log under a redirected HOME,
// header included. A missing file yields no rows.
func readTestLog(t *testing.T, home string) [][]string {
	t.Helper()
	f, err := os.Open(wantLogPath(home, testCSVFileName))
	if os.IsNotExist(err) {
		return nil
	}
	if err != nil {
		t.Fatalf("open test log: %v", err)
	}
	defer f.Close()
	rows, err := csv.NewReader(f).ReadAll()
	if err != nil {
		t.Fatalf("read test log: %v", err)
	}
	return rows
}

// stateWithTests builds the runtime state of a finished check carrying per-test
// records.
func stateWithTests(name string, records ...checks.TestRecord) *CheckState {
	return &CheckState{
		Definition: &checks.CheckDefinition{ID: "desktop-rust-tests", Nickname: name, App: checks.AppDesktop},
		Status:     StatusFailed,
		Tests:      records,
	}
}

func TestLogTestStatsWritesOneRowPerInterestingTest(t *testing.T) {
	home := redirectHome(t)

	logTestStats(stateWithTests("rust-tests",
		checks.TestRecord{ID: "cmdr_lib::a::fails", Outcome: checks.TestFailed, Seconds: 0.02, Attempt: 1},
		checks.TestRecord{ID: "cmdr_lib::b::flakes", Outcome: checks.TestFlaky, Seconds: 0.5, Attempt: 2},
		// A fast clean pass carries no signal for either question the log exists to
		// answer, and there are thousands of them per run.
		checks.TestRecord{ID: "cmdr_lib::c::is_fast", Outcome: checks.TestPassed, Seconds: 0.01, Attempt: 1},
		checks.TestRecord{ID: "cmdr_lib::d::is_slow", Outcome: checks.TestPassed, Seconds: 3.25, Attempt: 1},
	))

	rows := readTestLog(t, home)
	if len(rows) != 4 {
		t.Fatalf("expected a header plus 3 rows, got %d: %v", len(rows), rows)
	}
	if got := rows[0]; len(got) != len(testCSVHeader) || got[2] != "test_id" {
		t.Fatalf("unexpected header: %v", got)
	}
	// timestamp,check,test_id,status,duration_s,attempt
	want := [][]string{
		{"rust-tests", "cmdr_lib::a::fails", "fail", "0.020", "1"},
		{"rust-tests", "cmdr_lib::b::flakes", "flaky", "0.500", "2"},
		{"rust-tests", "cmdr_lib::d::is_slow", "pass", "3.250", "1"},
	}
	for i, expected := range want {
		got := rows[i+1][1:]
		if len(got) != len(expected) {
			t.Fatalf("row %d has %d fields, want %d: %v", i, len(got), len(expected), got)
		}
		for j := range expected {
			if got[j] != expected[j] {
				t.Errorf("row %d field %d: got %q, want %q", i, j, got[j], expected[j])
			}
		}
	}
}

func TestLogTestStatsKeepsAnUnknownDurationDistinctFromZero(t *testing.T) {
	home := redirectHome(t)

	logTestStats(stateWithTests("rust-tests",
		checks.TestRecord{ID: "cmdr_lib::a::no_timing", Outcome: checks.TestFailed, Seconds: -1, Attempt: 1},
	))

	rows := readTestLog(t, home)
	if len(rows) != 2 {
		t.Fatalf("expected a header plus 1 row, got %d: %v", len(rows), rows)
	}
	if got := rows[1][4]; got != "N/A" {
		t.Errorf("expected an unknown duration to read N/A, got %q", got)
	}
}

func TestLogTestStatsAppendsWithoutRepeatingTheHeader(t *testing.T) {
	home := redirectHome(t)

	logTestStats(stateWithTests("rust-tests",
		checks.TestRecord{ID: "cmdr_lib::a::fails", Outcome: checks.TestFailed, Seconds: 0.02, Attempt: 1}))
	logTestStats(stateWithTests("svelte-tests",
		checks.TestRecord{ID: "src/lib/a.test.ts::b", Outcome: checks.TestFailed, Seconds: 0.02, Attempt: 1}))

	rows := readTestLog(t, home)
	if len(rows) != 3 {
		t.Fatalf("expected a header plus 2 rows, got %d: %v", len(rows), rows)
	}
	if rows[2][1] != "svelte-tests" {
		t.Errorf("expected the second lane's row, got %v", rows[2])
	}
}

func TestLogTestStatsWritesNothingForANonTestCheck(t *testing.T) {
	home := redirectHome(t)

	logTestStats(stateWithTests("clippy"))

	if rows := readTestLog(t, home); len(rows) != 0 {
		t.Fatalf("expected no file at all, got %v", rows)
	}
}
