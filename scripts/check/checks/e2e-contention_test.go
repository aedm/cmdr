package checks

import (
	"errors"
	"strings"
	"testing"
)

// A fake isolated re-run. `stillFailing` is what the UNCHANGED-timeout stage reports,
// `withHeadroom` what the raised-timeout stage reports; a file absent from a map means
// everything in it passed at that stage.
type fakeE2ERerun struct {
	calls        []fakeE2ECall
	stillFailing map[string][]string
	withHeadroom map[string][]string
	err          error
}

type fakeE2ECall struct {
	file      string
	keys      []string
	timeoutMs int
}

func (f *fakeE2ERerun) rerun(target e2eFailingFile, timeoutMs int) (map[string]bool, error) {
	f.calls = append(f.calls, fakeE2ECall{file: target.file, keys: target.keys, timeoutMs: timeoutMs})
	if f.err != nil {
		return nil, f.err
	}
	source := f.stillFailing
	if timeoutMs > 0 {
		source = f.withHeadroom
	}
	failed := map[string]bool{}
	for _, key := range source[target.file] {
		failed[key] = true
	}
	return failed, nil
}

func specKey(file, title string) string { return file + "::::" + title }

func failingFile(file string, titles ...string) e2eFailingFile {
	keys := make([]string, 0, len(titles))
	for _, t := range titles {
		keys = append(keys, specKey(file, t))
	}
	return e2eFailingFile{file: file, shardName: "non-mtp-1", keys: keys}
}

func verdictByKey(results []E2ESpecResult) map[string]ContentionVerdict {
	byKey := map[string]ContentionVerdict{}
	for _, r := range results {
		byKey[r.Key] = r.Verdict
	}
	return byKey
}

// Passing alone once every shard has finished is the signal that the suite itself was
// starving the spec. No headroom stage should even happen.
func TestASpecThatPassesAloneAtTheSameTimeoutIsContention(t *testing.T) {
	f := &fakeE2ERerun{}
	results := ClassifyE2EContention([]e2eFailingFile{failingFile("dup.spec.ts", "duplicates a file")}, f.rerun, quiet)

	if len(results) != 1 {
		t.Fatalf("expected 1 result, got %+v", results)
	}
	if results[0].Verdict != VerdictContention {
		t.Errorf("verdict = %q, want %q", results[0].Verdict, VerdictContention)
	}
	if len(f.calls) != 1 {
		t.Fatalf("expected only the unchanged-timeout stage, got %+v", f.calls)
	}
	if f.calls[0].timeoutMs != 0 {
		t.Errorf("the first stage must leave the timeout alone, got %d", f.calls[0].timeoutMs)
	}
}

// Failing alone at the same timeout but passing with headroom means the spec got
// slower, not starved. That must NOT be absorbed as contention.
func TestASpecThatNeedsHeadroomAloneIsTooSlowNotContention(t *testing.T) {
	key := specKey("dup.spec.ts", "duplicates a file")
	f := &fakeE2ERerun{stillFailing: map[string][]string{"dup.spec.ts": {key}}}
	results := ClassifyE2EContention([]e2eFailingFile{failingFile("dup.spec.ts", "duplicates a file")}, f.rerun, quiet)

	if verdictByKey(results)[key] != VerdictTooSlow {
		t.Errorf("verdict = %q, want %q", verdictByKey(results)[key], VerdictTooSlow)
	}
	if len(f.calls) != 2 {
		t.Fatalf("expected both stages, got %+v", f.calls)
	}
	if f.calls[1].timeoutMs <= playwrightBaseTimeoutMs {
		t.Errorf("the headroom stage must raise the timeout past %d, got %d", playwrightBaseTimeoutMs, f.calls[1].timeoutMs)
	}
}

func TestASpecThatFailsEvenWithHeadroomIsARealFailure(t *testing.T) {
	key := specKey("dup.spec.ts", "duplicates a file")
	f := &fakeE2ERerun{
		stillFailing: map[string][]string{"dup.spec.ts": {key}},
		withHeadroom: map[string][]string{"dup.spec.ts": {key}},
	}
	results := ClassifyE2EContention([]e2eFailingFile{failingFile("dup.spec.ts", "duplicates a file")}, f.rerun, quiet)

	if verdictByKey(results)[key] != VerdictReal {
		t.Errorf("verdict = %q, want %q", verdictByKey(results)[key], VerdictReal)
	}
}

// The motivating scenario: David is still working on the machine while the re-run
// happens. "Needed headroom" then can't distinguish starvation from real slowness.
func TestNeedingHeadroomOnABusyMachineIsInconclusive(t *testing.T) {
	key := specKey("dup.spec.ts", "duplicates a file")
	f := &fakeE2ERerun{stillFailing: map[string][]string{"dup.spec.ts": {key}}}
	results := ClassifyE2EContention([]e2eFailingFile{failingFile("dup.spec.ts", "duplicates a file")}, f.rerun, busy)

	if verdictByKey(results)[key] != VerdictInconclusive {
		t.Errorf("verdict = %q, want %q", verdictByKey(results)[key], VerdictInconclusive)
	}
}

func TestABusyMachineStillReportsARealE2EFailure(t *testing.T) {
	key := specKey("dup.spec.ts", "duplicates a file")
	f := &fakeE2ERerun{
		stillFailing: map[string][]string{"dup.spec.ts": {key}},
		withHeadroom: map[string][]string{"dup.spec.ts": {key}},
	}
	results := ClassifyE2EContention([]e2eFailingFile{failingFile("dup.spec.ts", "duplicates a file")}, f.rerun, busy)

	if verdictByKey(results)[key] != VerdictReal {
		t.Errorf("a busy machine must not excuse a genuine failure, got %q", verdictByKey(results)[key])
	}
}

// The re-run unit is the spec FILE, because `fullyParallel: false` + `workers: 1` +
// one shared app instance let a test depend on the ones before it in its file. Two
// failures in one file must therefore cost ONE re-run carrying the whole file.
func TestTwoFailuresInOneFileShareASingleFileRerun(t *testing.T) {
	f := &fakeE2ERerun{}
	results := ClassifyE2EContention(
		[]e2eFailingFile{failingFile("dup.spec.ts", "duplicates a file", "duplicates a folder")},
		f.rerun, quiet)

	if len(results) != 2 {
		t.Fatalf("expected a verdict per failing test, got %+v", results)
	}
	if len(f.calls) != 1 {
		t.Fatalf("expected one file-level re-run, got %+v", f.calls)
	}
	if f.calls[0].file != "dup.spec.ts" || len(f.calls[0].keys) != 2 {
		t.Errorf("the re-run must carry the whole file, got %+v", f.calls[0])
	}
}

// Only the specs the first stage couldn't clear may be escalated; a file whose
// failures all passed alone is done.
func TestOnlyStillFailingFilesGetTheHeadroomStage(t *testing.T) {
	slowKey := specKey("slow.spec.ts", "waits for a walk")
	f := &fakeE2ERerun{stillFailing: map[string][]string{"slow.spec.ts": {slowKey}}}
	results := ClassifyE2EContention([]e2eFailingFile{
		failingFile("starved.spec.ts", "copies a file"),
		failingFile("slow.spec.ts", "waits for a walk"),
	}, f.rerun, quiet)

	byKey := verdictByKey(results)
	if byKey[specKey("starved.spec.ts", "copies a file")] != VerdictContention {
		t.Errorf("starved.spec.ts = %q", byKey[specKey("starved.spec.ts", "copies a file")])
	}
	if byKey[slowKey] != VerdictTooSlow {
		t.Errorf("slow.spec.ts = %q", byKey[slowKey])
	}

	var headroomFiles []string
	for _, c := range f.calls {
		if c.timeoutMs > 0 {
			headroomFiles = append(headroomFiles, c.file)
		}
	}
	if len(headroomFiles) != 1 || headroomFiles[0] != "slow.spec.ts" {
		t.Errorf("only the still-failing file belongs in the headroom stage, got %v", headroomFiles)
	}
}

// A re-run that couldn't run at all (a dead app, a wedged socket) is not evidence
// about any spec. It must never buy a red run a warn.
func TestAnE2ERerunThatCannotRunLeavesTheSpecsReal(t *testing.T) {
	f := &fakeE2ERerun{err: errors.New("the isolated re-run selected no tests")}
	results := ClassifyE2EContention([]e2eFailingFile{failingFile("dup.spec.ts", "duplicates a file")}, f.rerun, quiet)

	if results[0].Verdict != VerdictReal {
		t.Errorf("a runner-level failure must not soften the verdict, got %q", results[0].Verdict)
	}
}

// The re-run is bounded, and the bound is disclosed: a look at 5 of 20 red files
// must never read as a look at all of them.
func TestTooManyFailingFilesSkipTheRerunAndDiscloseTheCap(t *testing.T) {
	var many []e2eFailingFile
	for i := range MaxE2ERerunFiles + 1 {
		many = append(many, failingFile(string(rune('a'+i))+".spec.ts", "boom"))
	}
	f := &fakeE2ERerun{}
	results, skipped := MaybeClassifyE2EContention(many, f.rerun, quiet)

	if !skipped {
		t.Fatal("expected the re-run to be skipped past the cap")
	}
	if results != nil {
		t.Errorf("no results should be produced when skipped, got %+v", results)
	}
	if len(f.calls) != 0 {
		t.Errorf("nothing should have run, got %+v", f.calls)
	}

	note := E2ERerunSkippedNote(len(many))
	if !strings.Contains(note, "6") || !strings.Contains(note, "5") {
		t.Errorf("the note must disclose both the count and the cap: %q", note)
	}
}

func TestUnderTheCapTheE2ERerunProceeds(t *testing.T) {
	f := &fakeE2ERerun{}
	_, skipped := MaybeClassifyE2EContention([]e2eFailingFile{failingFile("dup.spec.ts", "x")}, f.rerun, quiet)
	if skipped {
		t.Fatal("a single failing file is well under the cap")
	}
}

// A run where every verdict is contention is the whole point: the lane softens to a
// warn that names the specs and says plainly they are not the reader's problem.
func TestAnAllContentionRunResolvesToAWarnThatNamesTheSpecs(t *testing.T) {
	f := &fakeE2ERerun{}
	result, err := resolveE2EFailure(
		errors.New("playwright E2E tests failed across 1 shard"),
		[]e2eFailingFile{failingFile("dup.spec.ts", "duplicates a file")},
		f.rerun, quiet, nil, nil)

	if err != nil {
		t.Fatalf("a starved run must not be an error, got: %v", err)
	}
	if result.Code != ResultWarning {
		t.Errorf("code = %v, want %v", result.Code, ResultWarning)
	}
	for _, want := range []string{"dup.spec.ts", "duplicates a file", "none confirmed a defect"} {
		if !strings.Contains(result.Message, want) {
			t.Errorf("message missing %q: %q", want, result.Message)
		}
	}
}

// One genuine failure alongside starved ones keeps the whole run red, and the
// original transcript survives so the failure is still diagnosable.
func TestOneRealE2EFailureKeepsTheRunRed(t *testing.T) {
	realKey := specKey("broken.spec.ts", "asserts the wrong thing")
	f := &fakeE2ERerun{
		stillFailing: map[string][]string{"broken.spec.ts": {realKey}},
		withHeadroom: map[string][]string{"broken.spec.ts": {realKey}},
	}
	_, err := resolveE2EFailure(
		errors.New("playwright E2E tests failed across 2 shards\n  Error: expected true"),
		[]e2eFailingFile{
			failingFile("starved.spec.ts", "copies a file"),
			failingFile("broken.spec.ts", "asserts the wrong thing"),
		}, f.rerun, quiet, nil, nil)

	if err == nil {
		t.Fatal("a real failure in the batch must keep the run red")
	}
	for _, want := range []string{"starved.spec.ts", "broken.spec.ts", "Error: expected true"} {
		if !strings.Contains(err.Error(), want) {
			t.Errorf("error missing %q:\n%s", want, err)
		}
	}
	// The first line is also what `check-log.csv` keeps, so it has to split the failures
	// into the reader's problem and the machine's on its own.
	headline, _, _ := strings.Cut(err.Error(), "\n")
	for _, want := range []string{"1 of 2", "survived an isolated re-run", "the other 1 was the lane starving itself"} {
		if !strings.Contains(headline, want) {
			t.Errorf("headline missing %q: %q", want, headline)
		}
	}
}

// With nothing to excuse, the headline must not imply that anything was.
func TestTheHeadlineSaysSoWhenEveryFailureIsReal(t *testing.T) {
	line := e2eRedHeadline([]E2ESpecResult{
		{Key: specKey("a.spec.ts", "x"), Verdict: VerdictReal},
		{Key: specKey("b.spec.ts", "y"), Verdict: VerdictTooSlow},
	}, 0.3)

	if !strings.Contains(line, "2 failing specs survived an isolated re-run, so they're real") {
		t.Errorf("headline = %q", line)
	}
	if strings.Contains(line, "starving") {
		t.Errorf("nothing was starved, so nothing should say so: %q", line)
	}
}

// A red run whose reports name no failing spec (a shard that died before the test
// phase) has nothing to re-run. It must be reported exactly as it arrived.
func TestARedRunWithNoIdentifiableSpecIsReportedAsIs(t *testing.T) {
	f := &fakeE2ERerun{}
	_, err := resolveE2EFailure(
		errors.New("playwright E2E tests failed across 1 shard\n  socket did not appear"),
		nil, f.rerun, quiet, nil, nil)

	if err == nil {
		t.Fatal("an unclassifiable red run is still a failure")
	}
	if !strings.Contains(err.Error(), "socket did not appear") {
		t.Errorf("the original output must survive:\n%s", err)
	}
	if len(f.calls) != 0 {
		t.Errorf("nothing to re-run, so nothing should have run: %+v", f.calls)
	}
}

// The summary is what an agent actually reads, so each verdict has to be stated in
// words that need no interpreting, with the load that produced it.
func TestE2EContentionSummaryStatesEachVerdict(t *testing.T) {
	results := []E2ESpecResult{
		{Key: specKey("a.spec.ts", "starved"), File: "a.spec.ts", ShardName: "non-mtp-1", Verdict: VerdictContention},
		{Key: specKey("b.spec.ts", "slow"), File: "b.spec.ts", ShardName: "non-mtp-2", Verdict: VerdictTooSlow},
		{Key: specKey("c.spec.ts", "murky"), File: "c.spec.ts", ShardName: "mtp", Verdict: VerdictInconclusive},
		{Key: specKey("d.spec.ts", "broken"), File: "d.spec.ts", ShardName: "mtp", Verdict: VerdictReal},
	}
	summary := E2EContentionSummary(results, nil, 9.4)

	// The wording has to sort the specs into "not your problem" and "your problem"
	// explicitly: naming a verdict is no use if the reader still has to interpret it.
	for _, want := range []string{"starved", "slow", "murky", "broken", "9.4", "NOT your problem", "genuine failure, YOUR problem"} {
		if !strings.Contains(summary, want) {
			t.Errorf("summary missing %q:\n%s", want, summary)
		}
	}
}

// Task 2: a failing spec's own record answers the first question an agent asks, so
// it belongs on the same line as the verdict.
func TestTheSummaryCarriesEachSpecsHistory(t *testing.T) {
	key := specKey("dup.spec.ts", "duplicates a file")
	summary := E2EContentionSummary(
		[]E2ESpecResult{{Key: key, File: "dup.spec.ts", ShardName: "non-mtp-1", Verdict: VerdictReal}},
		map[string]TestHistory{key: {Runs: 41, Failures: 3, LastFailure: "2026-09-16"}},
		0.4)

	for _, want := range []string{"41", "3", "2026-09-16"} {
		if !strings.Contains(summary, want) {
			t.Errorf("summary missing %q:\n%s", want, summary)
		}
	}
}

// A spec with no rows in the log is a first-time failure, which is itself the answer
// to "has this happened before". It must say so rather than printing nothing.
func TestASpecWithNoHistorySaysSo(t *testing.T) {
	key := specKey("new.spec.ts", "brand new")
	summary := E2EContentionSummary(
		[]E2ESpecResult{{Key: key, File: "new.spec.ts", ShardName: "mtp", Verdict: VerdictReal}},
		map[string]TestHistory{}, 0.4)

	if !strings.Contains(summary, "no earlier") {
		t.Errorf("summary should say the log has nothing on it:\n%s", summary)
	}
}

// Playwright's own key format carries the file first, and the whole file-granularity
// design rests on being able to get it back out.
func TestE2EKeyFileTakesTheSpecFile(t *testing.T) {
	cases := map[string]string{
		"dup.spec.ts::Duplicate in place::duplicates a file": "dup.spec.ts",
		"dup.spec.ts::::no describe block":                   "dup.spec.ts",
		"weird-but-keyless":                                  "weird-but-keyless",
	}
	for key, want := range cases {
		if got := e2eKeyFile(key); got != want {
			t.Errorf("e2eKeyFile(%q) = %q, want %q", key, got, want)
		}
	}
}

// The trap the Rust lane hit with `--run-ignored only`: a re-run whose filter selects
// nothing reports zero failures, which would read as "everything passed alone" and
// turn every real failure into a contention warn. It has to be a runner error.
func TestARerunThatRanNoneOfTheTargetsIsARunnerError(t *testing.T) {
	target := failingFile("dup.spec.ts", "duplicates a file")
	if rerunCoveredTargets(nil, target.keys) {
		t.Error("an empty report covers nothing")
	}
	if rerunCoveredTargets([]TestRecord{{ID: specKey("other.spec.ts", "unrelated")}}, target.keys) {
		t.Error("a report about other tests covers nothing")
	}
	if !rerunCoveredTargets([]TestRecord{{ID: target.keys[0], Outcome: TestPassed}}, target.keys) {
		t.Error("the target test ran, so the re-run is evidence")
	}
}

// Task 3: the lane yields to whoever is using the machine. The wrapper has to keep the
// real argv intact, and must degrade to the bare command when `nice` isn't there.
func TestNiceArgvWrapsTheCommandAndDegradesGracefully(t *testing.T) {
	got := niceArgv(true, "pnpm", []string{"exec", "playwright"})
	want := []string{"nice", "-n", "5", "pnpm", "exec", "playwright"}
	if strings.Join(got, " ") != strings.Join(want, " ") {
		t.Errorf("niceArgv = %v, want %v", got, want)
	}

	got = niceArgv(false, "pnpm", []string{"exec", "playwright"})
	if strings.Join(got, " ") != "pnpm exec playwright" {
		t.Errorf("without nice available the argv must be untouched, got %v", got)
	}
}
