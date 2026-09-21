package checks

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// runBarePollOn writes the supplied files into a temp repo layout rooted at
// `apps/desktop/test/e2e-playwright/` and runs the check.
func runBarePollOn(t *testing.T, files map[string]string) (CheckResult, error) {
	t.Helper()
	root := t.TempDir()
	specDir := filepath.Join(root, "apps", "desktop", "test", "e2e-playwright")
	if err := os.MkdirAll(specDir, 0o755); err != nil {
		t.Fatalf("mkdir: %v", err)
	}
	for rel, body := range files {
		full := filepath.Join(specDir, rel)
		if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
			t.Fatalf("mkdir: %v", err)
		}
		if err := os.WriteFile(full, []byte(body), 0o644); err != nil {
			t.Fatalf("write: %v", err)
		}
	}
	return RunBarePoll(&CheckContext{RootDir: root})
}

func TestBarePoll_FlagsBareAwaitPollUntil(t *testing.T) {
	_, err := runBarePollOn(t, map[string]string{
		"app.spec.ts": `
import { test } from './fixtures.js'
import { pollUntil } from './helpers.js'
test('foo', async () => {
    await pollUntil(page, () => check(), 3000)
})
`,
	})
	if err == nil {
		t.Fatal("expected violation, got success")
	}
	if !strings.Contains(err.Error(), "app.spec.ts:5") {
		t.Errorf("expected app.spec.ts:5, got: %s", err.Error())
	}
}

func TestBarePoll_FlagsBareAwaitPollFs(t *testing.T) {
	_, err := runBarePollOn(t, map[string]string{
		"mtp.spec.ts": `
test('foo', async () => {
    await pollFs(page, () => fs.existsSync('/x'), 5000)
})
`,
	})
	if err == nil {
		t.Fatal("expected violation, got success")
	}
}

func TestBarePoll_AcceptsExpectWrapped(t *testing.T) {
	res, err := runBarePollOn(t, map[string]string{
		"app.spec.ts": `
test('foo', async () => {
    expect(await pollUntil(page, () => check(), 3000)).toBe(true)
})
`,
	})
	if err != nil {
		t.Fatalf("expected success when wrapped in expect, got: %v", err)
	}
	if res.Code != ResultSuccess {
		t.Fatalf("expected ResultSuccess, got %v: %s", res.Code, res.Message)
	}
}

// The formatter decides whether `expect(` and the `await` share a line: once a wrapped
// call grows past the print width (which `waitBudget(...)` did to 4 real sites at once),
// oxfmt moves the `await` onto its own line and a start-of-line anchor reads it as bare.
// The value is just as consumed as before, so a reformat must never invent a violation.
func TestBarePoll_AcceptsExpectWrappedAcrossLines(t *testing.T) {
	res, err := runBarePollOn(t, map[string]string{
		"app.spec.ts": `
test('foo', async () => {
    expect(
        await pollUntil(page, async () => (await page.count(ROWS)) > 0, waitBudget(30000)),
    ).toBe(true)
})
`,
	})
	if err != nil {
		t.Fatalf("expected success when expect( is on the previous line, got: %v", err)
	}
	if res.Code != ResultSuccess {
		t.Fatalf("expected ResultSuccess, got %v: %s", res.Code, res.Message)
	}
}

// The other shapes a formatter can produce, all of which consume the value.
func TestBarePoll_AcceptsOtherLeadInsOnThePreviousLine(t *testing.T) {
	for name, body := range map[string]string{
		"argument": "await Promise.all([\n    pollUntil(page, () => a(), 3000),\n])",
		"assignment": "const ok =\n" +
			"    await pollUntil(page, () => reallyQuiteALongConditionName(), 3000)\nif (!ok) throw new Error('x')",
		"arrow body": "const probe = async () =>\n    await pollUntil(page, () => a(), 3000)\nawait probe()",
		"return":     "async function probe() {\n    return (\n        await pollUntil(page, () => a(), 3000)\n    )\n}",
	} {
		_, err := runBarePollOn(t, map[string]string{
			"app.spec.ts": "test('foo', async () => {\n" + body + "\n})\n",
		})
		if err != nil {
			t.Errorf("%s: expected success, got: %v", name, err)
		}
	}
}

// ...and the inverse: a genuinely bare call after a finished statement still fails,
// whatever the line before it happens to be.
func TestBarePoll_StillFlagsBareCallAfterAFinishedStatement(t *testing.T) {
	for name, prev := range map[string]string{
		"semicolon":    "await page.click(BUTTON);",
		"no semicolon": "await page.click(BUTTON)",
		"block close":  "if (x) {\n        await page.click(BUTTON)\n    }",
		"blank line":   "await page.click(BUTTON)\n",
	} {
		_, err := runBarePollOn(t, map[string]string{
			"app.spec.ts": "test('foo', async () => {\n    " + prev +
				"\n    await pollUntil(page, () => check(), 3000)\n})\n",
		})
		if err == nil {
			t.Errorf("%s: expected a violation for the bare call, got success", name)
		}
	}
}

func TestBarePoll_AcceptsAssignedToVariable(t *testing.T) {
	_, err := runBarePollOn(t, map[string]string{
		"app.spec.ts": `
test('foo', async () => {
    const ok = await pollUntil(page, () => check(), 3000)
    if (!ok) throw new Error('timeout')
})
`,
	})
	if err != nil {
		t.Fatalf("expected success when assigned to var, got: %v", err)
	}
}

func TestBarePoll_AcceptsReturnedFromHelper(t *testing.T) {
	_, err := runBarePollOn(t, map[string]string{
		"helpers.ts": `
export async function settle(page) {
    return await pollUntil(page, () => check(), 3000)
}
`,
	})
	if err != nil {
		t.Fatalf("expected success when returned, got: %v", err)
	}
}

func TestBarePoll_AcceptsInsideIfGuard(t *testing.T) {
	_, err := runBarePollOn(t, map[string]string{
		"app.spec.ts": `
test('foo', async () => {
    if (!(await pollUntil(page, () => check(), 3000))) throw new Error('x')
})
`,
	})
	if err != nil {
		t.Fatalf("expected success when wrapped in if-!, got: %v", err)
	}
}

func TestBarePoll_AllowsOptOutOnPreviousLine(t *testing.T) {
	res, err := runBarePollOn(t, map[string]string{
		"helpers.ts": `
async function dismissOverlay(page) {
    // allowed-bare-poll: best-effort cleanup of any lingering modal
    await pollUntil(page, async () => !(await page.isVisible('.modal-overlay')), 3000)
}
`,
	})
	if err != nil {
		t.Fatalf("expected success with opt-out, got: %v", err)
	}
	if res.Code != ResultSuccess {
		t.Fatalf("expected ResultSuccess, got %v: %s", res.Code, res.Message)
	}
}

func TestBarePoll_AllowsTrailingOptOut(t *testing.T) {
	_, err := runBarePollOn(t, map[string]string{
		"helpers.ts": `
async function dismissOverlay(page) {
    await pollUntil(page, async () => !(await page.isVisible('.modal-overlay')), 3000) // allowed-bare-poll: cleanup
}
`,
	})
	if err != nil {
		t.Fatalf("expected success with trailing opt-out, got: %v", err)
	}
}

func TestBarePoll_IgnoresCommentsAndDocs(t *testing.T) {
	res, err := runBarePollOn(t, map[string]string{
		"docs.ts": "// await pollUntil(page, () => check(), 3000) is the bad shape\n" +
			"/* await pollFs(page, () => fs.exists('/x'), 5000) is also bad */\n" +
			" * await pollUntil(page, () => check(), 3000) (jsdoc-style continuation)\n",
	})
	if err != nil {
		t.Fatalf("expected success when only comments mention the patterns, got: %v", err)
	}
	if res.Code != ResultSuccess {
		t.Fatalf("expected ResultSuccess, got %v: %s", res.Code, res.Message)
	}
}

func TestBarePoll_FlagsMultilineCallByOpeningLine(t *testing.T) {
	// The opener-line anchor catches multi-line poll bodies too; we report the
	// `await pollUntil(` line, not the closing paren a few lines down.
	_, err := runBarePollOn(t, map[string]string{
		"app.spec.ts": `
test('foo', async () => {
    await pollUntil(
        page,
        async () => check(),
        3000,
    )
})
`,
	})
	if err == nil {
		t.Fatal("expected violation on multi-line call, got success")
	}
	if !strings.Contains(err.Error(), "app.spec.ts:3") {
		t.Errorf("expected app.spec.ts:3 (the opener line), got: %s", err.Error())
	}
}

func TestBarePoll_FlagsAdditionalHelpers(t *testing.T) {
	_, err := runBarePollOn(t, map[string]string{
		"search.spec.ts": `
test('foo', async () => {
    await pollActiveMode(page, 'filename')
    await pollOverlayGone(page, 3000)
    await pollFocusedPane(page, 'left', 3000)
    await pollUntilValue(page, async () => 'x', 3000)
})
`,
	})
	if err == nil {
		t.Fatal("expected violations on the named helpers, got success")
	}
	for _, helper := range []string{"pollActiveMode", "pollOverlayGone", "pollFocusedPane", "pollUntilValue"} {
		if !strings.Contains(err.Error(), helper) {
			t.Errorf("expected violation mentioning %s, got: %s", helper, err.Error())
		}
	}
}

func TestBarePoll_SkipsNonTsFiles(t *testing.T) {
	res, err := runBarePollOn(t, map[string]string{
		"notes.md":  "await pollUntil(page, () => check(), 3000)\n",
		"data.json": `{"snippet": "await pollUntil(page, () => check(), 3000)"}` + "\n",
	})
	if err != nil {
		t.Fatalf("expected success on non-ts files, got: %v", err)
	}
	if res.Code != ResultSuccess {
		t.Fatalf("expected ResultSuccess, got %v: %s", res.Code, res.Message)
	}
}

func TestBarePoll_FlagsOrphanedOptOutOnPreviousLine(t *testing.T) {
	_, err := runBarePollOn(t, map[string]string{
		"helpers.ts": `
async function ready(page) {
    // allowed-bare-poll: stale reason, the call below was since refactored
    const ok = await pollUntil(page, () => check(), 3000)
    return ok
}
`,
	})
	if err == nil {
		t.Fatal("expected orphaned opt-out violation, got success")
	}
	if !strings.Contains(err.Error(), "unused") || !strings.Contains(err.Error(), "helpers.ts:3") {
		t.Errorf("expected unused-directive report at helpers.ts:3, got: %s", err.Error())
	}
}

func TestBarePoll_FlagsOrphanedTrailingOptOut(t *testing.T) {
	_, err := runBarePollOn(t, map[string]string{
		"helpers.ts": `
async function ready(page) {
    const ok = await pollUntil(page, () => check(), 3000) // allowed-bare-poll: stale
    return ok
}
`,
	})
	if err == nil {
		t.Fatal("expected orphaned trailing opt-out violation, got success")
	}
}

func TestBarePoll_IgnoresDirectiveMetaMention(t *testing.T) {
	res, err := runBarePollOn(t, map[string]string{
		"docs.ts": "// Opt out with `// allowed-bare-poll: <reason>` on the line above\n",
	})
	if err != nil {
		t.Fatalf("expected success for prose mention of the directive, got: %v", err)
	}
	if res.Code != ResultSuccess {
		t.Fatalf("expected ResultSuccess, got %v: %s", res.Code, res.Message)
	}
}

func TestBarePoll_PassesOnCleanCode(t *testing.T) {
	res, err := runBarePollOn(t, map[string]string{
		"clean.spec.ts": `
test('foo', async () => {
    expect(await pollUntil(page, () => check(), 3000)).toBe(true)
    const ready = await pollUntil(page, () => isReady(), 3000)
    expect(ready).toBe(true)
})
`,
	})
	if err != nil {
		t.Fatalf("expected success on clean code, got: %v", err)
	}
	if res.Code != ResultSuccess {
		t.Fatalf("expected ResultSuccess, got %v: %s", res.Code, res.Message)
	}
	if !strings.Contains(res.Message, "1 test file scanned") {
		t.Errorf("expected scanned count in success message, got: %s", res.Message)
	}
}
