package checks

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

// Turning a red Vitest run into something worth reading, for all three Vitest
// lanes (`desktop-svelte-tests`, `api-server-tests`, `dashboard-tests`).
//
// Vitest's default reporter prints a line per spec FILE, every test's console
// output, and every Svelte runtime warning, and THEN the failures. On the desktop
// suite that's ~1,300 lines to say one test went red, and pasting it into the
// check's error is what makes a reader reach for `tail` against the rule that
// says not to.
//
// The reporter already separates the two: everything from its first `⎯⎯ … ⎯⎯`
// banner onwards IS the failure account, so that's what the error carries. The
// transcript is saved to a log file the error names, the way the Playwright lane
// does it, so nothing is actually lost.
//
// ❗ Why the transcript and not the json report. The lane sets VITEST_JSON_REPORT
// (`vitest-test-log.go`) and that report names every failing test, but its
// `failureMessages` carry `error.stack`, and for a TIMEOUT Vitest's stack is a
// synthesized `Error: STACK_TRACE_ERROR` placeholder — the "Test timed out in
// 20000ms" line, the code frame, and the repo-relative paths exist only in the
// reporter's own output (verified against vitest 4.1.10, 2026-09-17). So the
// report is what counts and cross-checks the failures, and the transcript is what
// describes them.
//
// ❗ Fail open. A transcript with no failure section, an unreadable report, or a
// run that went red with no test failing (an error outside any test, a worker
// killed before it reported) prints the whole transcript. A filter that can hide
// a failure is worse than a verbose one.

// vitestTailSectionRE matches the banner above each of Vitest's tail sections:
// `⎯⎯⎯ Failed Tests 7 ⎯⎯⎯`, `⎯⎯ Unhandled Errors ⎯⎯`, `⎯⎯ Startup Error ⎯⎯`.
// Matching the SHAPE rather than the section names keeps whatever section Vitest
// grows next. The per-failure separator it writes BETWEEN failures (`⎯⎯⎯[1/7]⎯`)
// carries no spaces around its label, so it can't match and can't cut a section
// short.
var vitestTailSectionRE = regexp.MustCompile(`^⎯{2,} .+ ⎯{2,}$`)

// vitestFailure is one red test, or one spec that failed to collect at all.
type vitestFailure struct {
	// Spec is the spec's path relative to the app directory.
	Spec string
	// Name is the describe chain plus the test title. Empty for a file-level
	// failure, which belongs to no single test.
	Name string
	// Messages are the failure's errors as the json report carries them: each
	// error's stack, verbatim and never truncated. Only the fallback prints these;
	// the transcript's own section says it better.
	Messages []string
}

// collectVitestFailures pulls every failure out of a report: the failing
// assertions first, and a file-level message only for a failed spec that produced
// no failing assertion of its own (there, it's the whole story; beside real
// assertions it only repeats the first one).
func collectVitestFailures(report vitestJSONReport, appDir string) []vitestFailure {
	var failures []vitestFailure
	for _, file := range report.TestResults {
		spec := file.Name
		if rel, err := filepath.Rel(appDir, spec); err == nil {
			spec = rel
		}
		found := 0
		for _, assertion := range file.AssertionResults {
			if assertion.Status != "failed" {
				continue
			}
			found++
			failures = append(failures, vitestFailure{
				Spec:     spec,
				Name:     strings.Join(append(append([]string{}, assertion.AncestorTitles...), assertion.Title), " › "),
				Messages: assertion.FailureMessages,
			})
		}
		if found == 0 && file.Status == "failed" && strings.TrimSpace(file.Message) != "" {
			failures = append(failures, vitestFailure{Spec: spec, Messages: []string{file.Message}})
		}
	}
	return failures
}

// extractVitestFailureSections keeps the transcript from its first tail-section
// banner to the end. That's the reporter's own failure account; everything before
// it is the per-file progress and console output. Empty when the run printed no
// such section, which is the caller's signal to fall back.
func extractVitestFailureSections(cleanOutput string) string {
	lines := strings.Split(cleanOutput, "\n")
	for i, line := range lines {
		if vitestTailSectionRE.MatchString(strings.TrimSpace(line)) {
			return strings.TrimRight(strings.Join(lines[i:], "\n"), "\n")
		}
	}
	return ""
}

// renderVitestFailure builds the lane's red-run error message. A nil report means
// it couldn't be read; `cleanOutput` must already have ANSI stripped, and
// `transcriptPath` is where the full transcript was saved (empty when that save
// failed, which drops the pointer rather than the failure).
func renderVitestFailure(report *vitestJSONReport, appDir, cleanOutput, transcriptPath string) string {
	var failures []vitestFailure
	if report != nil {
		failures = collectVitestFailures(*report, appDir)
	}
	sections := extractVitestFailureSections(cleanOutput)
	if len(failures) == 0 && sections == "" {
		return renderVitestUnattributedFailure(report, cleanOutput)
	}

	var b strings.Builder
	b.WriteString(vitestFailureHeadline(report, failures, transcriptPath) + "\n")
	if diag := vitestRunDiagnostics(cleanOutput); diag != "" {
		b.WriteString("\n      run context:\n" + diag + "\n")
	}
	if sections != "" {
		b.WriteString("\n" + indentOutput(sections))
		return b.String()
	}
	// The report named failures the reporter didn't describe (a crash between the
	// two). Its stacks are second best, and second best beats silence.
	b.WriteString("\n      the run printed no failure section; from the test report instead:\n")
	for _, failure := range failures {
		b.WriteString("\n" + indentOutput("FAIL "+failure.Spec))
		if failure.Name != "" {
			b.WriteString(indentOutput("     " + failure.Name))
		}
		for _, message := range failure.Messages {
			b.WriteString(indentOutput(message))
		}
	}
	return b.String()
}

// vitestFailureHeadline is the message's first line: how many tests went red out
// of how many ran, and where the full transcript was kept. It leans on the report
// for the counts and says only what it can stand behind, because a wrong count
// beside a real failure reads as a broken check.
func vitestFailureHeadline(report *vitestJSONReport, failures []vitestFailure, transcriptPath string) string {
	var b strings.Builder
	switch {
	case len(failures) > 0:
		fmt.Fprintf(&b, "%d %s failed", len(failures), Pluralize(len(failures), "test", "tests"))
		if report != nil && report.NumTotalTests > 0 {
			fmt.Fprintf(&b, " of %s", formatThousands(report.NumTotalTests))
		}
	default:
		// A failure section with no report behind it: a startup error, or a crash
		// before the report was written. Don't invent a count for it.
		b.WriteString("the run failed")
	}
	if transcriptPath != "" {
		fmt.Fprintf(&b, " (full transcript: %s)", transcriptPath)
	}
	return b.String()
}

// renderVitestUnattributedFailure is the last fallback: the run went red, no test
// owns the failure, and the reporter printed no section about it. The transcript
// is the only account there is, so all of it goes through. The leading note says
// why, so a reader doesn't hunt for a failure section that was never written.
func renderVitestUnattributedFailure(report *vitestJSONReport, cleanOutput string) string {
	var b strings.Builder
	if report == nil {
		b.WriteString("the run left no readable test report and printed no failure section, so the whole transcript follows\n")
	} else {
		b.WriteString("the run failed with no test reporting a failure (an error outside any test), so the whole transcript follows\n")
	}
	if diag := vitestRunDiagnostics(cleanOutput); diag != "" {
		b.WriteString("\n      run context:\n" + diag + "\n")
	}
	b.WriteString("\n" + indentOutput(cleanOutput))
	return b.String()
}

// vitestWorkerDeathMarkers are the lines that mean a worker process died rather
// than reported. The first six are how Vitest words a worker it lost; the last two
// are how Node announces its own death, which is what an unhandled `'error'` event
// on a socket looks like from out here. Without them a crashed worker reads as
// silence: Vitest still prints a tally for the tests that did report.
var vitestWorkerDeathMarkers = []string{
	"Worker terminated",
	"Channel closed",
	"closed unexpectedly",
	"worker exited",
	"reached heap limit",
	"FATAL ERROR",
	"Unhandled 'error' event",
	"throw er;",
}

// vitestRunDiagnostics pulls the lines that reveal whether a run was complete: the
// `Test Files` / `Tests` tallies (a skip count above the usual handful means files
// didn't run, so coverage is unreliable) plus any worker death. Returned indented
// for the failure message. `cleanOutput` must already have ANSI stripped.
func vitestRunDiagnostics(cleanOutput string) string {
	var order []string
	seen := map[string]int{}
	for line := range strings.SplitSeq(cleanOutput, "\n") {
		t := strings.TrimSpace(line)
		keep := strings.HasPrefix(t, "Test Files ") || strings.HasPrefix(t, "Tests ")
		for _, marker := range vitestWorkerDeathMarkers {
			keep = keep || strings.Contains(t, marker)
		}
		if !keep {
			continue
		}
		// Four dead workers print the same two lines four times. Counting them says
		// the same thing in one line, and the count is the part worth reading.
		if _, dup := seen[t]; !dup {
			order = append(order, t)
		}
		seen[t]++
	}
	var out []string
	for _, line := range order {
		if n := seen[line]; n > 1 {
			line = fmt.Sprintf("%s (×%d)", line, n)
		}
		out = append(out, "      "+line)
	}
	return strings.Join(out, "\n")
}

// diagnoseVitestFailure is [renderVitestFailure] with its IO: it reads the run's
// report and saves the transcript for the message to point at. Every IO failure
// here is silent by design — the renderer already degrades to the full transcript,
// and instrumentation must never be the thing that changes a verdict.
func diagnoseVitestFailure(reportPath, appDir, rawOutput string) string {
	clean := StripANSI(rawOutput)
	report, err := readVitestReport(reportPath)
	if err != nil {
		report = nil
	}
	return renderVitestFailure(report, appDir, clean, saveVitestTranscript(clean))
}

// newVitestReportPath reserves a private path for one run's json report, and a
// cleanup that removes it. Private per invocation for the same reason the desktop
// lane's coverage directory is: two concurrent runs of the same lane must not read
// each other's report.
func newVitestReportPath(lane string) (path string, cleanup func(), err error) {
	dir, err := os.MkdirTemp("", "cmdr-vitest-report-"+lane+"-*")
	if err != nil {
		return "", func() {}, err
	}
	return filepath.Join(dir, "test-results.json"), func() { os.RemoveAll(dir) }, nil
}

// saveVitestTranscript writes the full transcript where it outlives the run's temp
// directory, and returns the path (empty when it couldn't be written). The name is
// run-scoped so `sweepStaleCheckArtifacts` collects it a week later.
func saveVitestTranscript(clean string) string {
	file, err := os.CreateTemp(os.TempDir(), fmt.Sprintf("cmdr-vitest-%d-*.log", os.Getpid()))
	if err != nil {
		return ""
	}
	defer file.Close()
	if _, err := file.WriteString(clean); err != nil {
		return ""
	}
	return file.Name()
}
