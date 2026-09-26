package main

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"cmdr/scripts/check/checks"
)

// outputCSVFileName is the FOURTH log: one row per check that ran, with how big
// its output was and how much of it reached stdout. It's its own file for the
// reason `test-log.csv` is (`check-log.csv` can't grow a column without breaking
// every reader of a long history). Schema and example queries:
// `scripts/check/DETAILS.md` § "Output budget and the full-output log".
const outputCSVFileName = "output-log.csv"

// outputDirName holds the full text of every failed or warning check (and of any
// output too big to print whole), one file per check run, beside the CSV logs.
const outputDirName = "output"

// outputKeep caps how many full-output files survive. The newest are kept; a
// red run leaves a handful, so 500 covers weeks of history in any worktree mix.
const outputKeep = 500

// The stdout budget per check, applied in quiet mode only (`-v` and `--ci` print
// everything). Agents read the run's stdout whole, so one noisy lane shouldn't
// spend their context: an output over either limit prints its first and last
// lines plus a pointer to the saved full text. Most checks print a few lines and
// never come near it.
const (
	outputBudgetLines = 40
	outputBudgetBytes = 8 * 1024
	excerptHeadLines  = 20
	excerptTailLines  = 8
	excerptLineRunes  = 300
)

var (
	outputCSVHeader = []string{"timestamp", "check", "result", "output_bytes", "output_lines", "printed_bytes", "full_output"}
	outputCSVMu     sync.Mutex
	outputDirMu     sync.Mutex
)

// overOutputBudget reports whether an output is too big to print whole.
func overOutputBudget(body string) bool {
	return len(body) > outputBudgetBytes || countLines(body) > outputBudgetLines
}

// countLines counts the lines of a check's output, a trailing newline not
// starting another one.
func countLines(body string) int {
	body = strings.TrimSuffix(body, "\n")
	if body == "" {
		return 0
	}
	return strings.Count(body, "\n") + 1
}

// capOutput returns what quiet mode prints for a check's output: the output
// itself when it fits the budget, otherwise its first and last lines (errors
// lead, and summaries like nextest's failure list trail) with a closing line
// that says how much was cut and where the whole of it is. fullPath is empty
// when nothing was saved (`--no-log`), and the pointer then names `-v` alone.
func capOutput(body, fullPath string) string {
	if !overOutputBudget(body) {
		return body
	}
	lines := strings.Split(strings.TrimSuffix(body, "\n"), "\n")
	var kept []string
	if len(lines) <= excerptHeadLines+excerptTailLines {
		kept = clipLines(lines)
	} else {
		omitted := len(lines) - excerptHeadLines - excerptTailLines
		kept = append(clipLines(lines[:excerptHeadLines]),
			fmt.Sprintf("⋯ %s more %s ⋯", checks.FormatThousands(omitted), checks.Pluralize(omitted, "line", "lines")))
		kept = append(kept, clipLines(lines[len(lines)-excerptTailLines:])...)
	}
	where := "rerun with -v to print it"
	if fullPath != "" {
		where = fmt.Sprintf("full output: %s (or rerun with -v)", fullPath)
	}
	kept = append(kept, fmt.Sprintf("Output cut to fit (%s lines, %s bytes in all); %s",
		checks.FormatThousands(len(lines)), checks.FormatThousands(len(body)), where))
	return strings.Join(kept, "\n")
}

// clipLines cuts each line to excerptLineRunes, so one minified blob can't blow
// the byte budget on its own. A clipped line gets a color reset, since the cut
// may have landed inside a colored span.
func clipLines(lines []string) []string {
	out := make([]string, len(lines))
	for i, line := range lines {
		if utf8.RuneCountInString(line) <= excerptLineRunes {
			out[i] = line
			continue
		}
		out[i] = string([]rune(line)[:excerptLineRunes]) + "…" + colorReset
	}
	return out
}

// outputBody is the text a check produced: its error for a failure, its message
// otherwise.
func outputBody(state *CheckState) string {
	if state.Status == StatusFailed && state.Error != nil {
		return state.Error.Error()
	}
	return state.Result.Message
}

// keepsFullOutput reports whether a check's full output is worth saving: a
// failure or a warning always (that's what someone comes back for), anything
// else only when stdout got an excerpt, so the pointer always has a target.
func keepsFullOutput(state *CheckState, body string) bool {
	if state.Status == StatusFailed || (state.Status == StatusCompleted && state.Result.Code == checks.ResultWarning) {
		return true
	}
	return overOutputBudget(body)
}

// saveFullOutput writes one check run's full output under the output dir and
// returns its path, or "" when the write failed (silently, like every log here:
// instrumentation must not colour a verdict). It prunes the dir to outputKeep.
func saveFullOutput(state *CheckState, rootDir, body string) string {
	csvPath, err := logPath(outputCSVFileName)
	if err != nil {
		return ""
	}
	dir := filepath.Join(filepath.Dir(csvPath), outputDirName)
	now := time.Now()
	// The timestamp leads so names sort chronologically, which retention relies
	// on; the pid keeps two worktrees running the same check apart.
	name := fmt.Sprintf("%s-%d-%s.log", now.Format("2006-01-02T15-04-05.000"), os.Getpid(), state.Definition.CLIName())
	path := filepath.Join(dir, name)
	header := fmt.Sprintf("# %s: %s, %s, in %s\n\n",
		state.Definition.CLIName(), resultLabel(state), now.Format("2006-01-02 15:04:05"), rootDir)

	outputDirMu.Lock()
	defer outputDirMu.Unlock()
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return ""
	}
	if err := os.WriteFile(path, []byte(header+body+"\n"), 0o644); err != nil {
		return ""
	}
	pruneOutputDir(dir, outputKeep)
	return path
}

// pruneOutputDir deletes the oldest files in dir until at most keep remain.
func pruneOutputDir(dir string, keep int) {
	entries, err := os.ReadDir(dir)
	if err != nil || len(entries) <= keep {
		return
	}
	names := make([]string, 0, len(entries))
	for _, e := range entries {
		if !e.IsDir() {
			names = append(names, e.Name())
		}
	}
	slices.Sort(names)
	for _, name := range names[:max(0, len(names)-keep)] {
		_ = os.Remove(filepath.Join(dir, name))
	}
}

// logOutputStats appends one row to the output log for a check that ran.
func logOutputStats(state *CheckState, body string, printedBytes int) {
	appendCSVRows(&outputCSVMu, outputCSVFileName, outputCSVHeader, [][]string{{
		time.Now().Format("2006-01-02 15:04:05"),
		state.Definition.CLIName(),
		resultLabel(state),
		fmt.Sprintf("%d", len(body)),
		fmt.Sprintf("%d", countLines(body)),
		fmt.Sprintf("%d", printedBytes),
		state.FullOutputPath,
	}})
}

// resultLabel names a check's outcome for the output log and the saved file's
// header. Unlike `check-log.csv`'s `result`, a warning is its own label here,
// because a warning is one of the two outputs this log exists to size.
func resultLabel(state *CheckState) string {
	switch state.Status {
	case StatusFailed:
		return "fail"
	case StatusSkipped:
		return "skip"
	case StatusBlocked:
		return "blocked"
	case StatusCached:
		return "cached"
	}
	if state.Result.Code == checks.ResultWarning {
		return "warn"
	}
	return "pass"
}
