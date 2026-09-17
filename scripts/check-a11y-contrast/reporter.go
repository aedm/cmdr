package main

import (
	"fmt"
	"sort"
)

const (
	colorRed    = "\033[31m"
	colorGreen  = "\033[32m"
	colorYellow = "\033[33m"
	colorDim    = "\033[2m"
	colorReset  = "\033[0m"
)

// Report prints violations to stdout in a human-readable format.
// Returns true if any violations were reported.
func Report(violations []Finding, warnings []string, rootDir string, verbose bool) bool {
	if len(violations) == 0 && len(warnings) == 0 {
		return false
	}

	// Sort: file, then line, then mode.
	sort.SliceStable(violations, func(i, j int) bool {
		if violations[i].File != violations[j].File {
			return violations[i].File < violations[j].File
		}
		if violations[i].Line != violations[j].Line {
			return violations[i].Line < violations[j].Line
		}
		return violations[i].Mode < violations[j].Mode
	})

	if len(violations) > 0 {
		fmt.Printf("%s=== Contrast violations ===%s\n", colorYellow, colorReset)
		for _, v := range violations {
			delta := v.Threshold - v.Ratio
			tag := ""
			if v.Placeholder {
				tag = " [placeholder]"
			}
			accent := ""
			if v.AccentVariant != "" {
				accent = fmt.Sprintf("  accent=%s", v.AccentVariant)
			}
			fmt.Printf(
				"  %s%s:%d%s  %s%s%s  mode=%s%s  fg=%s  bg=%s  ratio=%.2f  need=%.1f  delta=-%.2f%s\n",
				colorRed, RelPath(rootDir, v.File), v.Line, colorReset,
				colorDim, v.Selector, colorReset,
				v.Mode, accent,
				v.FG.Hex(), v.BG.Hex(),
				v.Ratio, v.Threshold, delta,
				tag,
			)
		}
		fmt.Println()
	}

	if verbose && len(warnings) > 0 {
		fmt.Printf("%s=== Warnings ===%s\n", colorYellow, colorReset)
		for _, w := range warnings {
			fmt.Printf("  %s%s%s\n", colorDim, w, colorReset)
		}
		fmt.Println()
	}

	return len(violations) > 0
}

// ReportOpacity prints every unmodeled opacity dimming found by
// `AnalyzeOpacity` and returns true if any were found. Unlike the WCAG and
// APCA gates, this is advisory, not a hard failure (see the exit-code
// contract in main.go's package doc comment): the tool can't verify a
// dimmed text color's real contrast without a browser, so a reported case
// might be a real bug or might be fine — it's surfaced for triage, not
// blocked on. Always printed regardless of exit code, so findings stay
// visible every run until each is converted to a color token (which lets
// the rule walker verify it directly) or hand-modeled in a synthesizer.
func ReportOpacity(findings []OpacityFinding, rootDir string) bool {
	if len(findings) == 0 {
		return false
	}

	sort.SliceStable(findings, func(i, j int) bool {
		if findings[i].File != findings[j].File {
			return findings[i].File < findings[j].File
		}
		return findings[i].Line < findings[j].Line
	})

	fmt.Printf("%s=== Unmodeled opacity (contrast not verified) ===%s\n", colorYellow, colorReset)
	for _, f := range findings {
		fmt.Printf(
			"  %s%s:%d%s  %s%s%s  opacity=%.2g\n",
			colorRed, RelPath(rootDir, f.File), f.Line, colorReset,
			colorDim, f.Selector, colorReset,
			f.Opacity,
		)
	}
	// Printed once, not per line: every finding above gets the same two fixes.
	fmt.Printf(
		"  %sfix: express as a color token (e.g. --color-text-quiet) so the walker sees it, or hand-model it in a synthesizer (dropdown_states.go / query_dialog_states.go)%s\n",
		colorDim, colorReset,
	)
	fmt.Println()

	return true
}

// ReportStaleOpacityExemptions prints every per-element opacity exemption that
// excused nothing this run. It returns nothing (unlike its siblings above,
// which report their own count): the caller holds the slice and decides the
// exit code from it. Unlike the opacity findings above this IS a hard failure:
// a stale entry is something the tool can prove on its own, and left alone it
// fails silently — the element the
// entry covered reappears as a plain finding with nothing pointing at the
// hand-verified reason it used to carry (see `StaleOpacityExemption`). The
// `why` text is printed with each one, since that's the piece that would
// otherwise be lost, and it's usually enough to recognize where the element
// moved to.
func ReportStaleOpacityExemptions(stale []StaleOpacityExemption) {
	if len(stale) == 0 {
		return
	}

	fmt.Printf("%s=== Stale opacity exemptions (matched nothing) ===%s\n", colorRed, colorReset)
	for _, s := range stale {
		fmt.Printf(
			"  %s%s%s  %s%s%s  in %s%s%s\n",
			colorRed, s.Entry.fileSuffix, colorReset,
			colorDim, s.Entry.selector, colorReset,
			colorDim, s.List, colorReset,
		)
		fmt.Printf("    %swas exempt because: %s%s\n", colorDim, s.Entry.why, colorReset)
	}
	fmt.Printf(
		"  %sfix: the element moved, was renamed, or dropped its opacity. Find where it went and repoint the entry (re-read the markup first, the reason above is not proof it's still decorative), or delete the entry.%s\n",
		colorDim, colorReset,
	)
	fmt.Println()
}

// Summary returns a one-line summary for the final status line.
func Summary(fileCount, ruleCount, findingCount, violationCount int) string {
	return fmt.Sprintf(
		"%d %s, %d %s checked, %d %s evaluated, %d %s",
		fileCount, plural(fileCount, "file", "files"),
		ruleCount, plural(ruleCount, "rule", "rules"),
		findingCount, plural(findingCount, "pair", "pairs"),
		violationCount, plural(violationCount, "violation", "violations"),
	)
}

func plural(n int, s, p string) string {
	if n == 1 {
		return s
	}
	return p
}

// joinWarnings collapses duplicate warnings into a sorted deduplicated list.
func joinWarnings(ws []string) []string {
	seen := make(map[string]int, len(ws))
	for _, w := range ws {
		seen[w]++
	}
	out := make([]string, 0, len(seen))
	for w, n := range seen {
		if n > 1 {
			out = append(out, fmt.Sprintf("%s (x%d)", w, n))
		} else {
			out = append(out, w)
		}
	}
	sort.Strings(out)
	return out
}
