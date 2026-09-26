package main

import (
	"encoding/csv"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"cmdr/scripts/check/checks"
)

// numberedLines builds an n-line body whose lines can be told apart, so a test
// can say exactly which ones made it to stdout.
func numberedLines(n int) string {
	lines := make([]string, n)
	for i := range lines {
		lines[i] = fmt.Sprintf("finding %03d", i+1)
	}
	return strings.Join(lines, "\n")
}

// readOutputLog returns the rows of the output log under a redirected HOME,
// header included. A missing file yields no rows.
func readOutputLog(t *testing.T, home string) [][]string {
	t.Helper()
	f, err := os.Open(wantLogPath(home, outputCSVFileName))
	if os.IsNotExist(err) {
		return nil
	}
	if err != nil {
		t.Fatalf("open output log: %v", err)
	}
	defer f.Close()
	rows, err := csv.NewReader(f).ReadAll()
	if err != nil {
		t.Fatalf("read output log: %v", err)
	}
	return rows
}

// runOne runs a single check through a real Runner and returns what it printed.
func runOne(t *testing.T, quiet, noLog bool, run checks.CheckFunc) string {
	t.Helper()
	def := checks.CheckDefinition{
		ID: "desktop-i18n-coverage", Nickname: "i18n-coverage", DisplayName: "i18n coverage",
		App: checks.AppDesktop, Tech: "🌐 i18n", CpuWeight: 1, Run: run,
	}
	r := NewRunner(&checks.CheckContext{RootDir: t.TempDir()}, []checks.CheckDefinition{def}, nil, false, noLog, quiet)
	return captureStdout(t, func() { r.Run() })
}

func failingWith(body string) checks.CheckFunc {
	return func(*checks.CheckContext) (checks.CheckResult, error) { return checks.CheckResult{}, errors.New(body) }
}

func TestCapOutputLeavesShortOutputAlone(t *testing.T) {
	body := numberedLines(outputBudgetLines)
	if got := capOutput(body, "/logs/x.log"); got != body {
		t.Errorf("an output within budget must print verbatim, got:\n%s", got)
	}
}

func TestCapOutputKeepsTheHeadAndTailAndPointsAtTheFullOutput(t *testing.T) {
	got := capOutput(numberedLines(200), "/logs/x.log")

	for _, want := range []string{"finding 001", fmt.Sprintf("finding %03d", excerptHeadLines), "finding 200", "/logs/x.log", "-v"} {
		if !strings.Contains(got, want) {
			t.Errorf("capped output lacks %q:\n%s", want, got)
		}
	}
	if strings.Contains(got, "finding 100") {
		t.Errorf("capped output still carries the middle:\n%s", got)
	}
	if n := strings.Count(got, "\n") + 1; n > outputBudgetLines {
		t.Errorf("capped output is %d lines, over the %d-line budget:\n%s", n, outputBudgetLines, got)
	}
	// The reader has to know how much they're not seeing.
	if !strings.Contains(got, "200 lines") {
		t.Errorf("capped output doesn't say how long the full one is:\n%s", got)
	}
}

func TestCapOutputClipsOverlongLines(t *testing.T) {
	got := capOutput(strings.Repeat("x", 3*outputBudgetBytes), "/logs/x.log")
	if len(got) > outputBudgetBytes {
		t.Errorf("one giant line printed %d bytes, over the %d-byte budget", len(got), outputBudgetBytes)
	}
	if !strings.Contains(got, "/logs/x.log") {
		t.Errorf("clipped output doesn't point at the full one:\n%s", got)
	}
}

func TestCapOutputWithoutAFileStillSaysHowToSeeEverything(t *testing.T) {
	got := capOutput(numberedLines(200), "")
	if !strings.Contains(got, "-v") {
		t.Errorf("with logging off, the pointer must name -v:\n%s", got)
	}
}

func TestAFailedCheckSavesItsFullOutputAndPrintsAnExcerpt(t *testing.T) {
	home := redirectHome(t)
	body := numberedLines(200)

	printed := runOne(t, true, false, failingWith(body))

	if strings.Contains(printed, "finding 100") {
		t.Errorf("quiet mode printed the whole failure:\n%s", printed)
	}
	rows := readOutputLog(t, home)
	if len(rows) != 2 {
		t.Fatalf("expected a header plus 1 row, got %v", rows)
	}
	// timestamp,check,result,output_bytes,output_lines,printed_bytes,full_output
	row := rows[1]
	if row[1] != "i18n-coverage" || row[2] != "fail" {
		t.Errorf("unexpected row: %v", row)
	}
	if row[3] != fmt.Sprint(len(body)) || row[4] != "200" {
		t.Errorf("row records %s bytes / %s lines, want %d / 200", row[3], row[4], len(body))
	}
	if row[5] == "0" || row[5] == row[3] {
		t.Errorf("printed_bytes = %s, want the excerpt's size (under %s)", row[5], row[3])
	}
	saved := row[6]
	if !strings.HasPrefix(saved, filepath.Join(filepath.Dir(wantLogPath(home, outputCSVFileName)), outputDirName)) {
		t.Fatalf("full output saved outside the log dir: %q", saved)
	}
	if !strings.Contains(printed, saved) {
		t.Errorf("stdout doesn't point at %s:\n%s", saved, printed)
	}
	content, err := os.ReadFile(saved)
	if err != nil {
		t.Fatalf("read saved output: %v", err)
	}
	if !strings.Contains(string(content), body) {
		t.Errorf("saved file lacks the full output:\n%s", content)
	}
}

func TestVerbosePrintsTheWholeFailure(t *testing.T) {
	redirectHome(t)
	printed := runOne(t, false, false, failingWith(numberedLines(200)))
	if !strings.Contains(printed, "finding 100") {
		t.Errorf("-v must print the full output:\n%s", printed)
	}
}

func TestAWarningIsSavedToo(t *testing.T) {
	home := redirectHome(t)
	runOne(t, true, false, func(*checks.CheckContext) (checks.CheckResult, error) {
		return checks.CheckResult{Code: checks.ResultWarning, Message: "2 things:\n- a\n- b", Total: -1, Issues: 2, Changes: -1}, nil
	})
	rows := readOutputLog(t, home)
	if len(rows) != 2 || rows[1][2] != "warn" || rows[1][6] == "" {
		t.Fatalf("expected one saved warn row, got %v", rows)
	}
}

func TestAPassLogsItsSizeButSavesNothing(t *testing.T) {
	home := redirectHome(t)
	runOne(t, true, false, func(*checks.CheckContext) (checks.CheckResult, error) {
		return checks.Success("all clean"), nil
	})
	rows := readOutputLog(t, home)
	if len(rows) != 2 {
		t.Fatalf("expected a header plus 1 row, got %v", rows)
	}
	// Quiet mode collapses a clean pass, so nothing of it reached stdout.
	if row := rows[1]; row[2] != "pass" || row[3] != "9" || row[5] != "0" || row[6] != "" {
		t.Errorf("unexpected pass row: %v", row)
	}
	if _, err := os.Stat(filepath.Join(filepath.Dir(wantLogPath(home, outputCSVFileName)), outputDirName)); !os.IsNotExist(err) {
		t.Errorf("a pass must not create the output dir (stat err: %v)", err)
	}
}

func TestNoLogWritesNothingAndPointsAtVerbose(t *testing.T) {
	home := redirectHome(t)
	printed := runOne(t, true, true, failingWith(numberedLines(200)))
	if rows := readOutputLog(t, home); len(rows) != 0 {
		t.Errorf("--no-log wrote output rows: %v", rows)
	}
	if !strings.Contains(printed, "-v") {
		t.Errorf("with nothing saved, the excerpt must point at -v:\n%s", printed)
	}
}

func TestPruneOutputDirKeepsTheNewest(t *testing.T) {
	dir := t.TempDir()
	// Names sort chronologically, which is the order retention relies on.
	for i := range outputKeep + 5 {
		name := fmt.Sprintf("2026-09-27T10-00-%05d-1-check.log", i)
		if err := os.WriteFile(filepath.Join(dir, name), nil, 0o644); err != nil {
			t.Fatal(err)
		}
	}

	pruneOutputDir(dir, outputKeep)

	entries, err := os.ReadDir(dir)
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != outputKeep {
		t.Fatalf("kept %d files, want %d", len(entries), outputKeep)
	}
	if entries[0].Name() != "2026-09-27T10-00-00005-1-check.log" {
		t.Errorf("oldest kept file = %s, want the five oldest gone", entries[0].Name())
	}
}
