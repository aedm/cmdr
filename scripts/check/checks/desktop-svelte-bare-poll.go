package checks

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
)

// AllowBarePollComment is the magic comment that opts a single line out of the
// bare-poll check. Place it on the line immediately above the flagged line, or
// at the end of the flagged line, with a short reason.
//
//	// allowed-bare-poll: best-effort cleanup of any lingering modal
//	await pollUntil(tauriPage, async () => !(await tauriPage.isVisible('.modal-overlay')), 3000)
const AllowBarePollComment = "// allowed-bare-poll:"

// barePollHelpers is the set of `Promise<boolean>` polling helpers whose return
// value is the success signal. Awaiting them as a bare expression statement
// silently swallows timeouts: the helper returns `false`, no assertion ever
// runs, and the test passes green. The fix is either to use Playwright's
// `expect.poll(...).toBeTruthy()` (preferred) or to wrap the call in
// `expect(await ...).toBe(true)`.
//
// Add new helpers here as they're introduced.
var barePollHelpers = []string{
	"pollUntil",
	"pollFs",
	"pollUntilValue",
	"pollOverlayGone",
	"pollFocusedPane",
	"pollActiveMode",
}

// barePollRegex matches `await <helper>(` at the start of a line (after any
// indent). That's the bare-expression-statement shape: the return value isn't
// assigned, returned, wrapped in `expect(...)`, or guarded by an `if`. Same-
// line lead-ins like `const x = await foo(` or `expect(await foo(` have
// non-whitespace before `await`, so the start-of-line anchor excludes them.
//
// ❗ The anchor alone is NOT the whole test, because which line the lead-in sits on is
// the FORMATTER's choice, not the author's: an `expect(await pollUntil(…))` that grows
// past the print width becomes `expect(\n    await pollUntil(…),\n).toBe(true)`, and the
// value is just as consumed as before. Wrapping every budget in `waitBudget(…)` did that
// to four real sites at once. So a match here still has to clear
// `barePollContinuesPreviousLine` before it counts.
var barePollRegex = regexp.MustCompile(
	`^\s*await\s+(` + strings.Join(barePollHelpers, "|") + `)\s*\(`,
)

// barePollContinuationSuffixes end a line that is mid-expression, so whatever follows is
// an operand rather than a new statement. A statement that FINISHED ends in `;`, `)`,
// `{`, or `}`, none of which are here.
var barePollContinuationSuffixes = []string{"(", "[", ",", "=>", "=", "&&", "||", "??", "?", ":", "+", "return"}

// barePollContinuesPreviousLine reports whether the previous line of code left an
// expression open, which makes the `await` on this line an operand of it.
func barePollContinuesPreviousLine(prevCode string) bool {
	trimmed := strings.TrimRight(prevCode, " \t")
	if trimmed == "" {
		return false
	}
	for _, suffix := range barePollContinuationSuffixes {
		if strings.HasSuffix(trimmed, suffix) {
			return true
		}
	}
	return false
}

type barePollSite struct {
	relPath string
	line    int
	helper  string
	text    string
}

// RunBarePoll fails the build if any test file under `apps/desktop/test/` calls
// one of the known `Promise<boolean>` polling helpers as a bare expression
// statement (return value discarded). The convention is documented in
// `apps/desktop/test/e2e-playwright/CLAUDE.md` § "Polling helpers".
func RunBarePoll(ctx *CheckContext) (CheckResult, error) {
	testDir := filepath.Join(ctx.RootDir, "apps", "desktop", "test")

	violations, orphans, scanned, err := scanForBarePoll(ctx.RootDir, testDir)
	if err != nil {
		return CheckResult{}, fmt.Errorf("failed to scan test files: %w", err)
	}

	var parts []string
	if len(violations) > 0 {
		sort.Slice(violations, func(i, j int) bool {
			if violations[i].relPath == violations[j].relPath {
				return violations[i].line < violations[j].line
			}
			return violations[i].relPath < violations[j].relPath
		})
		var sb strings.Builder
		for _, v := range violations {
			sb.WriteString(fmt.Sprintf("  %s:%d: %s\n", v.relPath, v.line, v.text))
		}
		parts = append(parts, fmt.Sprintf(
			"found %d bare `await <pollHelper>(...)` %s where the return value is "+
				"discarded (the test silently passes if the poll times out). "+
				"Prefer Playwright's `expect.poll(() => ...).toBeTruthy()`, or wrap as "+
				"`expect(await pollUntil(...)).toBe(true)`. "+
				"Add `%s <reason>` on the line above to opt a specific site out (rare):\n%s",
			len(violations), Pluralize(len(violations), "site", "sites"), AllowBarePollComment, strings.TrimRight(sb.String(), "\n"),
		))
	}
	if len(orphans) > 0 {
		parts = append(parts, formatOrphanDirectives(AllowBarePollComment, orphans))
	}
	if len(parts) > 0 {
		return CheckResult{}, fmt.Errorf("%s", strings.Join(parts, "\n"))
	}

	return Success(fmt.Sprintf(
		"%d test %s scanned, no bare polling-helper calls",
		scanned, Pluralize(scanned, "file", "files"),
	)), nil
}

func scanForBarePoll(rootDir, testDir string) ([]barePollSite, []orphanDirective, int, error) {
	var violations []barePollSite
	var orphans []orphanDirective
	scanned := 0

	err := filepath.WalkDir(testDir, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			// Skip generated / heavy directories.
			if d.Name() == "node_modules" || d.Name() == "test-results" || d.Name() == "playwright-report" {
				return filepath.SkipDir
			}
			return nil
		}
		// Scan only TypeScript files; the patterns are TS-specific.
		if !strings.HasSuffix(d.Name(), ".ts") {
			return nil
		}
		scanned++

		relPath, relErr := filepath.Rel(rootDir, path)
		if relErr != nil {
			relPath = path
		}

		fileViolations, fileOrphans, scanErr := scanFileForBarePoll(path, relPath)
		if scanErr != nil {
			return scanErr
		}
		violations = append(violations, fileViolations...)
		orphans = append(orphans, fileOrphans...)
		return nil
	})

	return violations, orphans, scanned, err
}

// scanFileForBarePoll walks one file's lines, tracking just enough context to tell a
// bare expression statement from an operand the formatter happened to wrap.
func scanFileForBarePoll(path, relPath string) ([]barePollSite, []orphanDirective, error) {
	var violations []barePollSite

	f, openErr := os.Open(path)
	if openErr != nil {
		return nil, nil, openErr
	}
	defer f.Close()

	scanner := bufio.NewScanner(f)
	scanner.Buffer(make([]byte, 64*1024), 1024*1024)
	tracker := newDirectiveTracker(AllowBarePollComment, "//")
	// `prev` is the line immediately above, comment or not, because that is where an
	// opt-out directive lives. `prevCode` skips comments and blanks, because what
	// decides whether this `await` starts a statement is the last real CODE above it.
	var prev, prevCode string
	lineNum := 0
	for scanner.Scan() {
		lineNum++
		line := scanner.Text()
		tracker.observe(lineNum, line)

		site, isComment := classifyBarePollLine(line, prev, prevCode, tracker, lineNum)
		if site != nil {
			site.relPath = relPath
			violations = append(violations, *site)
		}
		prev = line
		if !isComment && strings.TrimSpace(line) != "" {
			prevCode = line
		}
	}
	return violations, tracker.orphans(relPath), scanner.Err()
}

// classifyBarePollLine decides what one line is. It returns a site (minus its path) when
// the line is a genuinely bare poll, and reports whether the line was a comment, which
// the caller needs to keep `prevCode` pointing at the last real code.
func classifyBarePollLine(
	line, prev, prevCode string,
	tracker *directiveTracker,
	lineNum int,
) (site *barePollSite, isComment bool) {
	trimmed := strings.TrimLeft(line, " \t")
	if strings.HasPrefix(trimmed, "//") || strings.HasPrefix(trimmed, "*") {
		return nil, true
	}

	m := barePollRegex.FindStringSubmatch(line)
	if m == nil {
		return nil, false
	}

	// An operand of an expression the previous line left open, not a statement of its
	// own: which line the lead-in sits on is the formatter's choice, not the author's.
	if barePollContinuesPreviousLine(prevCode) {
		return nil, false
	}

	// Opt-out: `// allowed-bare-poll: <reason>` on the previous line OR as a trailing
	// comment on the same line.
	if hasAllowBarePollComment(prev) || hasAllowBarePollComment(line) {
		tracker.markUsed(lineNum, line, prev)
		return nil, false
	}

	return &barePollSite{
		line:   lineNum,
		helper: m[1],
		text:   strings.TrimSpace(line),
	}, false
}

func hasAllowBarePollComment(line string) bool {
	return strings.Contains(line, AllowBarePollComment)
}
