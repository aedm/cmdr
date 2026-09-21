package checks

import (
	"fmt"
	"strings"
)

// Contention-aware re-run for the Rust suite. The verdict vocabulary, the busy/quiet
// judgement, and the warn rule are shared with the Playwright lane and live in
// `contention-verdict.go`; this file is the Rust half.
//
// The problem: on a saturated machine the global 8 s nextest cap kills CPU-bound tests
// that would finish in milliseconds on an idle one. Measured 2026-07-29 on an M3 Max
// (16 cores) at load ~198, a full `rust-tests` run produced 13 failures, 9 of them cap
// kills of pure-compute tests (`find_newlines_utf8_matches_memchr`,
// `walk_memory_tests::*`, `tar_each_codec_round_trips_a_file`, …). Those tests are not
// wrong and their deadlines are not wrong; they simply could not get 8 s of wall-clock
// while 200 threads fought over 16 cores.
//
// Loosening the cap globally is the wrong fix: it costs every idle run its hang
// detector, and the cap encodes a real incident (see `.config/nextest.toml`). Instead,
// a red run re-runs ONLY the failing tests, alone, and lets the outcome classify them.

// MaxContentionRerun bounds the re-run. Past it the machine was too loaded for the
// result to mean anything, and re-running hundreds of tests serially is its own problem.
// Real saturated runs have produced up to 13 failures at once (measured at load ~198),
// so 15 clears observed reality with headroom while still refusing a runaway.
const MaxContentionRerun = 15

// nextest profiles the two re-run stages use. Defined in `.config/nextest.toml`.
const (
	// ContentionProbeProfile keeps every deadline exactly as the failing run had them
	// and only removes the parallelism. That's what makes a pass here mean "starved".
	ContentionProbeProfile = "contention-probe"
	// ContentionRetryProfile grants headroom. It deliberately does NOT `inherit` the
	// default profile: an inherited per-test override BEATS a profile-level
	// `slow-timeout` (verified against nextest 0.9.136, 2026-07-29), so inheriting
	// would silently keep the tight per-test caps this stage exists to lift.
	ContentionRetryProfile = "contention-retry"
)

// ContentionResult is one failing test plus what the re-runs concluded.
type ContentionResult struct {
	Binary  string
	Name    string
	Class   FailureClass
	Verdict ContentionVerdict
}

// ContentionRunner runs the named tests under one nextest profile and returns the raw
// output. Injected so the classification logic is testable without a cargo build.
type ContentionRunner func(profile string, names []string) (string, error)

// RealFailures drops leaks. nextest counts a leaky test as PASSED (it appears in the
// "N passed (M leaky)" tally), so re-running one is meaningless and counting one as a
// failure overstates a red run.
func RealFailures(failures []RustFailure) []RustFailure {
	real := make([]RustFailure, 0, len(failures))
	for _, f := range failures {
		if f.Class != ClassLeak {
			real = append(real, f)
		}
	}
	return real
}

// MaybeClassifyContention runs the two-stage classification unless the failure count is
// past MaxContentionRerun, in which case it reports `skipped` so the caller can disclose
// the cap instead of silently examining a subset.
func MaybeClassifyContention(failures []RustFailure, run ContentionRunner, load LoadSampler) (results []ContentionResult, skipped bool) {
	if len(failures) > MaxContentionRerun {
		return nil, true
	}
	return ClassifyContention(failures, run, load), false
}

// ClassifyContention re-runs the failing tests alone, first at their original deadlines
// and then (only for those still failing) with headroom.
func ClassifyContention(failures []RustFailure, run ContentionRunner, load LoadSampler) []ContentionResult {
	if len(failures) == 0 {
		return nil
	}

	results := make([]ContentionResult, 0, len(failures))
	index := map[string]int{}
	names := make([]string, 0, len(failures))
	for _, f := range failures {
		index[f.Name] = len(results)
		results = append(results, ContentionResult{
			Binary:  f.Binary,
			Name:    f.Name,
			Class:   f.Class,
			Verdict: VerdictContention, // upgraded below if it keeps failing
		})
		names = append(names, f.Name)
	}

	probeOut, err := run(ContentionProbeProfile, names)
	if err != nil {
		// A runner-level problem (couldn't launch cargo) isn't evidence about any test.
		// Leave every verdict real rather than inventing an excuse for a red run.
		return markAll(results, VerdictReal)
	}
	stillFailing := failedNames(probeOut)
	if len(stillFailing) == 0 {
		return results // everything passed alone at the unchanged deadline
	}

	// Sample load around the escalation run: that verdict is the only one whose meaning
	// depends on the machine being quiet.
	headroomVerdict := VerdictTooSlow
	if load() > BusyLoadPerCore {
		headroomVerdict = VerdictInconclusive
	}
	for _, n := range stillFailing {
		if i, ok := index[n]; ok {
			results[i].Verdict = headroomVerdict
		}
	}

	retryOut, err := run(ContentionRetryProfile, stillFailing)
	if err != nil {
		return markNames(results, index, stillFailing, VerdictReal)
	}
	return markNames(results, index, failedNames(retryOut), VerdictReal)
}

func markAll(results []ContentionResult, v ContentionVerdict) []ContentionResult {
	for i := range results {
		results[i].Verdict = v
	}
	return results
}

func markNames(results []ContentionResult, index map[string]int, names []string, v ContentionVerdict) []ContentionResult {
	for _, n := range names {
		if i, ok := index[n]; ok {
			results[i].Verdict = v
		}
	}
	return results
}

// failedNames reuses the shared classifier so a re-run's failures are recognised exactly
// as the main run's are, leaks included (and therefore excluded).
func failedNames(output string) []string {
	failures := RealFailures(ClassifyRustFailures(output))
	names := make([]string, 0, len(failures))
	for _, f := range failures {
		names = append(names, f.Name)
	}
	return names
}

// RustVerdicts projects the results onto the shared vocabulary, which is what
// `WarnOnly` judges. Both lanes carry their own result shape (a Rust test has a binary
// and a deadline class, a spec has a file and a shard) and neither is worth forcing
// onto the other.
func RustVerdicts(results []ContentionResult) []ContentionVerdict {
	verdicts := make([]ContentionVerdict, 0, len(results))
	for _, r := range results {
		verdicts = append(verdicts, r.Verdict)
	}
	return verdicts
}

// ContentionSummary renders the verdicts for a human or agent reading the check output.
func ContentionSummary(results []ContentionResult, loadAvg float64) string {
	var contention, tooSlow, inconclusive, real []string
	for _, r := range results {
		switch r.Verdict {
		case VerdictContention:
			contention = append(contention, r.Name)
		case VerdictTooSlow:
			tooSlow = append(tooSlow, r.Name)
		case VerdictInconclusive:
			inconclusive = append(inconclusive, r.Name)
		default:
			real = append(real, r.Name)
		}
	}

	var b strings.Builder
	fmt.Fprintf(&b, "%d %s re-run alone (load was %.1f):\n",
		len(results), Pluralize(len(results), "test", "tests"), loadAvg)
	section := func(heading string, names []string) {
		if len(names) == 0 {
			return
		}
		b.WriteString("  • " + heading + "\n")
		for _, n := range names {
			b.WriteString("      - " + n + "\n")
		}
	}
	section(fmt.Sprintf("%d passed alone at the same deadline, so the suite was starving %s: contention, not a defect",
		len(contention), Pluralize(len(contention), "it", "them")), contention)
	section("Needed extra headroom even alone on a quiet machine, so this is real slowness: tweak the test or give it an explicit per-test override",
		tooSlow)
	section("Needed extra headroom, but the machine was still busy during the re-run, so starvation and real slowness can't be told apart. Re-run on a quiet machine to settle it",
		inconclusive)
	section("Still failing alone with headroom: a genuine failure", real)
	return b.String()
}

// ContentionSkippedNote explains why no re-run happened, disclosing the cap so a reader
// never mistakes a bounded look for a full one.
func ContentionSkippedNote(failed int) string {
	return fmt.Sprintf(
		"%d tests failed, past the %d-test contention re-run cap, so no isolated re-run was attempted. "+
			"That many failures at once usually means the machine was too loaded for the run to mean anything; "+
			"re-run the suite on a quieter machine.",
		failed, MaxContentionRerun)
}

// NextestFilterExpr builds an exact-match filter for the named tests.
func NextestFilterExpr(names []string) string {
	parts := make([]string, 0, len(names))
	for _, n := range names {
		parts = append(parts, fmt.Sprintf("test(=%s)", n))
	}
	return strings.Join(parts, " + ")
}
