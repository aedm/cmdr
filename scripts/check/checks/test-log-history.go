package checks

import (
	"encoding/csv"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"time"
)

// Reading a named test's record back out of the per-test log
// (`~/.local/share/check-runner/cmdr/test-log.csv`, written by `logTestStats` in
// `stats.go`; schema and retention in `scripts/check/DETAILS.md` § "The per-test log").
//
// A red lane's first question is always "has this one gone red before?", and the log
// has answered it since long before anything read it back. This is the reader.
//
// Two guardrails carry over from the writing side:
//
//   - **Instrumentation never colours a verdict.** A missing, slow, or malformed log
//     returns an error the caller can mention and move past. ❌ Never a partial count
//     presented as the whole record: rows are appended chronologically, so a read that
//     gave up early is missing exactly the recent ones a reader cares about.
//   - **❌ Never add a column to that log.** Every reader of it hard-errors on a
//     field-count mismatch, and it is past 200,000 rows. This reader resolves its
//     columns by HEADER NAME so an appended column can never silently shift the read.
//
// The log is streamed, never slurped: only the handful of requested ids accumulate.

// TestHistory is one test's record across every run the log kept.
type TestHistory struct {
	// Runs is how many rows the log holds for this test. It is NOT how many times the
	// test ran: clean passes under `testLogSlowSeconds` are never written, so absence
	// means "fast, or never ran". Rendered with that caveat attached.
	Runs int
	// Failures counts `fail` and `timeout` rows. A timeout is a failure with a cause.
	Failures int
	// Flakes counts `flaky` rows: red on the first attempt, rescued by a retry.
	Flakes int
	// LastFailure is the `YYYY-MM-DD` of the most recent failure, empty when there is none.
	LastFailure string
}

// testLogHistoryBudget bounds the read. The whole 200,000-row file streams in well
// under a second, so blowing past this means something is wrong with the disk or the
// file, and a red lane must not wait on it.
const testLogHistoryBudget = 5 * time.Second

// TestLogPath resolves the per-test log the same way `stats.go`'s `logPath` does,
// honouring $XDG_DATA_HOME. The logs live outside the repo on purpose: they're a
// measurement history spanning years and worktrees, and a worktree teardown must never
// take them with it.
func TestLogPath() (string, error) {
	dataHome := os.Getenv("XDG_DATA_HOME")
	if dataHome == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			return "", err
		}
		dataHome = filepath.Join(home, ".local", "share")
	}
	return filepath.Join(dataHome, "check-runner", "cmdr", "test-log.csv"), nil
}

// LookupTestHistory streams the per-test log and returns the record of each named test.
// A test with no rows is absent from the map, which is itself the answer ("never seen
// before"). Asking for nothing opens nothing.
//
// Ids are matched across every lane that logged them, not just the calling one: both
// E2E lanes run the same specs from the same files, and a spec flaking on Linux is
// evidence about the spec.
func LookupTestHistory(path string, ids []string, budget time.Duration) (map[string]TestHistory, error) {
	if len(ids) == 0 {
		return map[string]TestHistory{}, nil
	}

	wanted := make(map[string]bool, len(ids))
	for _, id := range ids {
		wanted[id] = true
	}

	f, err := os.Open(path)
	if err != nil {
		return nil, fmt.Errorf("read the per-test log: %w", err)
	}
	defer f.Close()

	r := csv.NewReader(f)
	// The log's rows are uniform, but a torn final row (the runner hard-killed mid-write)
	// must not throw away everything read before it.
	r.ReuseRecord = true

	header, err := r.Read()
	if err != nil {
		return nil, fmt.Errorf("read the per-test log's header: %w", err)
	}
	cols, err := resolveTestLogColumns(header)
	if err != nil {
		return nil, err
	}

	return foldTestLogRows(r, cols, wanted, budget)
}

// testLogColumns is where the three fields this reader needs sit in the log's header.
type testLogColumns struct{ id, status, timestamp int }

// resolveTestLogColumns locates them by NAME, which is what makes an appended column
// harmless instead of a silent one-field shift in everything read after it.
func resolveTestLogColumns(header []string) (testLogColumns, error) {
	cols := testLogColumns{id: -1, status: -1, timestamp: -1}
	for i, name := range header {
		switch name {
		case "test_id":
			cols.id = i
		case "status":
			cols.status = i
		case "timestamp":
			cols.timestamp = i
		}
	}
	if cols.id < 0 || cols.status < 0 || cols.timestamp < 0 {
		return cols, errors.New("the per-test log's header names no test_id/status/timestamp column")
	}
	return cols, nil
}

// outOfRange reports whether a row is too short to hold every column this reader reads.
func (c testLogColumns) outOfRange(row []string) bool {
	return c.id >= len(row) || c.status >= len(row) || c.timestamp >= len(row)
}

// foldTestLogRows streams what's left of the log, accumulating only the wanted ids.
func foldTestLogRows(
	r *csv.Reader,
	cols testLogColumns,
	wanted map[string]bool,
	budget time.Duration,
) (map[string]TestHistory, error) {
	history := map[string]TestHistory{}
	deadline := time.Now().Add(budget)
	for rows := 0; ; rows++ {
		// Checking the clock per row would cost more than the parse; every 4,096 is
		// fine grained enough for a budget measured in seconds.
		if rows%4096 == 0 && time.Now().After(deadline) {
			return nil, fmt.Errorf("reading the per-test log took longer than %s", budget)
		}
		row, err := r.Read()
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			// A malformed row says nothing about the rows around it; the counts stay
			// honest by skipping it rather than abandoning the read.
			if errors.Is(err, csv.ErrFieldCount) {
				continue
			}
			return nil, fmt.Errorf("read the per-test log: %w", err)
		}
		if cols.outOfRange(row) || !wanted[row[cols.id]] {
			continue
		}
		countTestLogRow(history, row, cols)
	}
	return history, nil
}

// countTestLogRow folds one row into the running record of the test it names.
func countTestLogRow(history map[string]TestHistory, row []string, cols testLogColumns) {
	id := row[cols.id]
	h := history[id]
	h.Runs++
	switch TestOutcome(row[cols.status]) {
	case TestFailed, TestTimedOut:
		h.Failures++
		if day := logDay(row[cols.timestamp]); day > h.LastFailure {
			h.LastFailure = day
		}
	case TestFlaky:
		h.Flakes++
	}
	history[id] = h
}

// logDay takes the date out of a `YYYY-MM-DD HH:MM:SS` stamp. An unexpected shape is
// returned whole rather than truncated into a wrong-looking date.
func logDay(timestamp string) string {
	if len(timestamp) >= 10 && timestamp[4] == '-' && timestamp[7] == '-' {
		return timestamp[:10]
	}
	return timestamp
}

// renderTestHistory is the phrase that goes beside a failing test. It always discloses
// that `Runs` counts logged rows rather than runs, because the alternative reads as a
// pass rate and isn't one.
func renderTestHistory(h TestHistory, found bool) string {
	if !found || h.Runs == 0 {
		return "no earlier rows in the per-test log"
	}
	out := fmt.Sprintf("%d %s logged", h.Failures, Pluralize(h.Failures, "failure", "failures"))
	if h.Flakes > 0 {
		out += fmt.Sprintf(" and %d %s", h.Flakes, Pluralize(h.Flakes, "flake", "flakes"))
	}
	out += fmt.Sprintf(" in %d %s", h.Runs, Pluralize(h.Runs, "row", "rows"))
	if h.LastFailure != "" {
		out += ", last failed " + h.LastFailure
	}
	return out
}

// testHistoryCaveat explains the denominator once, under the listing, so no reader
// mistakes "3 failures in 41 rows" for a 7% failure rate.
const testHistoryCaveat = "Rows are what the per-test log keeps: every failure and flake, plus passes over 1 s. " +
	"A spec with few rows may simply be fast."
