package checks

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// Fixtures are verbatim vitest 4.1.10 output, captured by running
// `src/lib/licensing/licensing.a11y.test.ts` with its 20 s budget cut to 50 ms so
// the real timeout failure from the lane could be reproduced (2026-09-17). Keep
// them verbatim when the pinned vitest version moves — written-from-memory output
// is how a parser passes its tests and matches nothing real.

// realVitestTranscript is one red run's whole output, shortened to one failure
// and one progress block. The `⎯⎯⎯[1/7]⎯` separator, the `❯` frame markers and
// the two-space tally gaps are all as vitest prints them.
const realVitestTranscript = ` RUN  v4.1.10 /Users/x/cmdr/apps/desktop
      Coverage enabled with v8

 ❯ src/lib/licensing/licensing.a11y.test.ts (12 tests | 7 failed) 5400ms
     ✓ personal license (no status) has no a11y violations 143ms
     × details state (subscription with expiry) has no a11y violations 19ms

 Test Files  1 failed (1)
      Tests  7 failed | 5 passed (12)
   Start at  14:52:01
   Duration  9.20s

⎯⎯⎯⎯⎯⎯⎯ Failed Tests 7 ⎯⎯⎯⎯⎯⎯⎯

 FAIL  src/lib/licensing/licensing.a11y.test.ts > AcknowledgementsDialog a11y > has no a11y violations once the package lists are rendered
Error: Test timed out in 50ms.
If this is a long-running test, pass a timeout value as the last argument or configure it globally with "testTimeout".
 ❯ src/lib/licensing/licensing.a11y.test.ts:198:3
    197|   // else. ❗ The budget is the only thing raised — the assertion is un…
    198|   it('has no a11y violations once the package lists are rendered', { t…
       |   ^

⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯[1/7]⎯
`

func TestExtractVitestFailureSections_KeepsTheFailuresAndDropsTheProgress(t *testing.T) {
	got := extractVitestFailureSections(realVitestTranscript)

	// The whole point: the message a reader needs survives, and the json report
	// can't supply it (a timeout's stack is a STACK_TRACE_ERROR placeholder).
	if !strings.Contains(got, "Error: Test timed out in 50ms.") {
		t.Errorf("dropped the timeout message, which is the only readable account of it:\n%s", got)
	}
	if !strings.Contains(got, "licensing.a11y.test.ts:198:3") {
		t.Errorf("dropped the code frame:\n%s", got)
	}
	if !strings.HasPrefix(got, "⎯⎯⎯⎯⎯⎯⎯ Failed Tests 7 ⎯⎯⎯⎯⎯⎯⎯") {
		t.Errorf("should start at the section banner, got:\n%s", got)
	}
	for _, noise := range []string{"RUN  v4.1.10", "Coverage enabled", "✓ personal license"} {
		if strings.Contains(got, noise) {
			t.Errorf("kept pre-section noise %q:\n%s", noise, got)
		}
	}
}

func TestExtractVitestFailureSections_TheInterFailureSeparatorCannotCutASectionShort(t *testing.T) {
	// `⎯⎯⎯[1/7]⎯` has no spaces around its label, which is the only thing keeping
	// it out of the banner regex. If it ever matched, every failure after the
	// first would be dropped.
	if vitestTailSectionRE.MatchString("⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯[1/7]⎯") {
		t.Error("the inter-failure separator matched the tail-section banner")
	}
	if !strings.Contains(extractVitestFailureSections(realVitestTranscript), "[1/7]") {
		t.Error("the separator should survive inside the kept section")
	}
}

func TestExtractVitestFailureSections_KeepsASectionItHasNeverHeardOf(t *testing.T) {
	// Matching the banner's shape rather than a list of names is what makes this
	// survive a vitest that grows a new section.
	out := extractVitestFailureSections("noise\n⎯⎯⎯ Unhandled Errors ⎯⎯⎯\nAggregateError [ECONNREFUSED]\n")
	if !strings.Contains(out, "AggregateError") || strings.Contains(out, "noise") {
		t.Errorf("unexpected section extraction:\n%s", out)
	}
}

func TestExtractVitestFailureSections_EmptyWhenThereIsNoSection(t *testing.T) {
	if got := extractVitestFailureSections("all green\nTests  12 passed (12)\n"); got != "" {
		t.Errorf("want empty so the caller falls back, got:\n%s", got)
	}
}

// realVitestReport is the json report the same run wrote, cut to the two shapes
// that matter: a failing assertion whose stack is the placeholder, and the
// tallies. `failureMessages` really does read `Error: STACK_TRACE_ERROR` for a
// timeout; that is the reason the transcript is what gets printed.
const realVitestReport = `{
  "numTotalTests": 12,
  "numPassedTests": 5,
  "numFailedTests": 7,
  "testResults": [
    {
      "name": "/Users/x/cmdr/apps/desktop/src/lib/licensing/licensing.a11y.test.ts",
      "status": "failed",
      "message": "",
      "assertionResults": [
        {
          "ancestorTitles": ["AcknowledgementsDialog a11y"],
          "title": "has no a11y violations once the package lists are rendered",
          "status": "failed",
          "duration": 51,
          "failureMessages": ["Error: STACK_TRACE_ERROR\n    at task (file:///x/@vitest/runner/dist/chunk-artifact.js:1784:27)"]
        },
        {
          "ancestorTitles": ["AcknowledgementsDialog a11y"],
          "title": "personal license (no status) has no a11y violations",
          "status": "passed",
          "duration": 143,
          "failureMessages": []
        }
      ]
    }
  ]
}`

func mustParseVitestReport(t *testing.T, body string) *vitestJSONReport {
	t.Helper()
	path := filepath.Join(t.TempDir(), "test-results.json")
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatalf("write report: %v", err)
	}
	report, err := readVitestReport(path)
	if err != nil {
		t.Fatalf("readVitestReport: %v", err)
	}
	return report
}

func TestRenderVitestFailure_TheWholeMessageForOneRedTest(t *testing.T) {
	report := mustParseVitestReport(t, realVitestReport)

	got := renderVitestFailure(report, "/Users/x/cmdr/apps/desktop", realVitestTranscript, "/tmp/cmdr-vitest-1-2.log")

	for _, want := range []string{
		"1 test failed of 12",             // the report's count, not a guess from text
		"/tmp/cmdr-vitest-1-2.log",        // nothing is lost, it just moved
		"Tests  7 failed | 5 passed (12)", // run context says the run itself was odd
		"Error: Test timed out in 50ms.",  // the failure, in words
		"licensing.a11y.test.ts:198:3",    // and where
	} {
		if !strings.Contains(got, want) {
			t.Errorf("missing %q from:\n%s", want, got)
		}
	}
	if strings.Contains(got, "STACK_TRACE_ERROR") {
		t.Errorf("printed the report's placeholder stack over the real message:\n%s", got)
	}
	// The count comes from the report's failing assertions; the transcript's own
	// "Failed Tests 7" belongs to the full file and isn't what we counted.
	if strings.Contains(got, "7 tests failed of") {
		t.Errorf("counted from the transcript instead of the report:\n%s", got)
	}
}

func TestRenderVitestFailure_FallsBackToTheWholeTranscriptWhenNothingOwnsTheFailure(t *testing.T) {
	// A run that dies before any test reports: no report, no failure section. The
	// transcript is the only account, so it all goes through.
	transcript := "node:events:505\n    throw er; // Unhandled 'error' event\nAggregateError [ECONNREFUSED]:\n"

	got := renderVitestFailure(nil, "/app", transcript, "")

	if !strings.Contains(got, "AggregateError [ECONNREFUSED]") {
		t.Errorf("swallowed the only account of the failure:\n%s", got)
	}
	if !strings.Contains(got, "whole transcript follows") {
		t.Errorf("should say why it's printing everything:\n%s", got)
	}
}

func TestRenderVitestFailure_UsesTheReportWhenTheReporterPrintedNoSection(t *testing.T) {
	// The report named a failure the reporter never described. Its stacks are
	// second best, and second best beats silence.
	report := mustParseVitestReport(t, realVitestReport)

	got := renderVitestFailure(report, "/Users/x/cmdr/apps/desktop", "Tests  7 failed | 5 passed (12)\n", "")

	if !strings.Contains(got, "FAIL src/lib/licensing/licensing.a11y.test.ts") {
		t.Errorf("should name the failing spec, relative to the app dir:\n%s", got)
	}
	if !strings.Contains(got, "AcknowledgementsDialog a11y › has no a11y violations once the package lists are rendered") {
		t.Errorf("should name the failing test:\n%s", got)
	}
}

func TestCollectVitestFailures_AFileThatNeverCollectedStillCounts(t *testing.T) {
	// A bad import fails the spec with zero assertions. Counting only assertions
	// would report an empty red run.
	report := mustParseVitestReport(t, `{
	  "numTotalTests": 0,
	  "testResults": [{
	    "name": "/app/src/broken.test.ts",
	    "status": "failed",
	    "message": "Error: Cannot find module './gone'",
	    "assertionResults": []
	  }]
	}`)

	failures := collectVitestFailures(*report, "/app")

	if len(failures) != 1 {
		t.Fatalf("want the collection failure, got %d: %+v", len(failures), failures)
	}
	if failures[0].Spec != "src/broken.test.ts" || failures[0].Name != "" {
		t.Errorf("a file-level failure belongs to no test: %+v", failures[0])
	}
	if !strings.Contains(failures[0].Messages[0], "Cannot find module") {
		t.Errorf("lost the only description of it: %+v", failures[0])
	}
}

func TestCollectVitestFailures_TheFileMessageDoesNotDoubleUpOnRealAssertions(t *testing.T) {
	// Beside failing assertions the file message only repeats the first one.
	report := mustParseVitestReport(t, realVitestReport)

	failures := collectVitestFailures(*report, "/Users/x/cmdr/apps/desktop")

	if len(failures) != 1 {
		t.Fatalf("want exactly the one failing assertion, got %d: %+v", len(failures), failures)
	}
}

func TestVitestRunDiagnostics_CountsRepeatedWorkerDeathsInsteadOfRepeatingThem(t *testing.T) {
	// Four dead workers printed the same two lines four times in the run that
	// prompted this. One line with a count says it better.
	transcript := strings.Repeat("node:events:505\n    throw er; // Unhandled 'error' event\n", 4) +
		"Tests  1 failed | 12705 passed | 6 skipped (12712)\n"

	got := vitestRunDiagnostics(transcript)

	if !strings.Contains(got, "throw er; // Unhandled 'error' event (×4)") {
		t.Errorf("want one counted line, got:\n%s", got)
	}
	if !strings.Contains(got, "Tests  1 failed | 12705 passed | 6 skipped (12712)") {
		t.Errorf("dropped the tally, which is how a reader spots an incomplete run:\n%s", got)
	}
	if strings.Count(got, "throw er") != 1 {
		t.Errorf("repeated a line it had already counted:\n%s", got)
	}
}

func TestVitestRunDiagnostics_SeesANodeProcessDyingOnAnUnhandledErrorEvent(t *testing.T) {
	// Pre-fix this said nothing: none of the old markers covered Node's own death
	// banner, so a crashed worker read as silence beside a green-looking tally.
	if got := vitestRunDiagnostics("node:events:505\n    throw er; // Unhandled 'error' event\n"); got == "" {
		t.Error("a dead worker went unreported")
	}
}

func TestSaveVitestTranscriptIsSweptLikeEveryOtherRunScopedArtifact(t *testing.T) {
	// The name has to match the sweeper's pattern or the transcripts pile up in
	// /tmp forever, which is the thing that sweep exists to prevent.
	path := saveVitestTranscript("some transcript")
	if path == "" {
		t.Fatal("could not save a transcript")
	}
	t.Cleanup(func() { _ = os.Remove(path) })

	if name := filepath.Base(path); !checkArtifactIsSweepable(name) {
		t.Errorf("%q does not match any sweep pattern, so nothing will ever collect it", name)
	}
}
