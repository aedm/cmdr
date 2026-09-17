package main

import "strings"

// OpacityFinding flags a rule that dims (or fully hides) text via a static
// `opacity: N < 1` the rule walker never sees: `analyzer.go` pairs a `color`
// and a `background` declared on the SAME selector and has no notion of
// `opacity` at all, so a translucent text color composites against whatever
// is behind it (its own ancestor's bg, at runtime) without this checker ever
// evaluating the result. This is a structural "we can't see this" flag, not
// a computed WCAG/APCA verdict like `Finding` — see the README's "Detect,
// don't compute" note.
type OpacityFinding struct {
	File     string
	Line     int
	Selector string
	Opacity  float64
}

// opacityInactiveMarkers are selector substrings that mark a disabled or
// otherwise inactive UI component. WCAG 1.4.3 explicitly exempts "inactive
// user interface components" from the text contrast requirement, and this is
// where the large majority of the codebase's `opacity` dimming lives
// (buttons, inputs, toggles, menu items). Checked against the lowercased
// selector text for attribute/pseudo-class forms.
var opacityInactiveMarkers = []string{
	":disabled",
	"[disabled]",
	"aria-disabled",
	"data-disabled",
	// A gated (locked/paywalled) feature card: its controls are genuinely
	// inactive while gated, same as `:disabled` (`SectionCard.svelte`).
	"data-gated",
}

// opacityInactiveClassWords are class-name STATE WORDS that mark the same
// disabled/inactive semantics as `opacityInactiveMarkers`, but via a plain
// class rather than an attribute/pseudo-class: bare `.<word>`, `.is-<word>`
// (also matches `.is-<word>-look`), or `.*-<word>`. `disabled` covers
// `.disabled`, `.is-disabled`, `.is-disabled-look`, `.text-field-disabled`.
// `unavailable` covers `.volume-item.is-unavailable` (a device the daemon
// lists but can't use — present, explained by its tooltip, `aria-disabled`
// on the row, and not openable: the same WCAG 1.4.3 inactive-component case
// as `disabled`, just a different word for it).
var opacityInactiveClassWords = []string{"disabled", "unavailable"}

func opacityInactiveSelector(rule Rule) bool {
	sel := strings.ToLower(rule.Selector)
	for _, marker := range opacityInactiveMarkers {
		if strings.Contains(sel, marker) {
			return true
		}
	}
	for _, c := range rule.Classes {
		cl := strings.ToLower(c)
		for _, word := range opacityInactiveClassWords {
			if cl == word || strings.HasPrefix(cl, "is-"+word) || strings.HasSuffix(cl, "-"+word) {
				return true
			}
		}
	}
	return false
}

// opacityDraggingMarkers are selector substrings marking transient
// drag-in-progress visual feedback: `.foo.is-dragging` (the ghosted row at
// the drag source) or `.foo.cannot-drop` (a drop target/cursor showing it
// can't accept the drop). Both dim ONLY while a pointer drag is in flight —
// WCAG 1.4.3's "incidental text" carve-out covers momentary UI feedback like
// this the same way it covers a hover tooltip or an animating toast, so it's
// exempt on the same footing as the disabled/inactive case, not because it's
// decorative. Checked the same way as `opacityInactiveMarkers` (a substring
// match against the lowercased selector), so any component's `.is-dragging`
// or `.cannot-drop` state matches without a per-component entry.
var opacityDraggingMarkers = []string{
	"is-dragging",
	"cannot-drop",
}

func opacityIsDraggingFeedback(rule Rule) bool {
	sel := strings.ToLower(rule.Selector)
	for _, marker := range opacityDraggingMarkers {
		if strings.Contains(sel, marker) {
			return true
		}
	}
	return false
}

// opacityDecorativeEntry is one hand-verified non-text element: an <Icon>
// component, an empty CSS-shape status indicator (a colored dot/bar/swatch
// with no child content), or an aria-hidden punctuation divider carrying no
// informational content. WCAG's text contrast requirement (and this
// checker's stated scope) is about TEXT; none of these render a text glyph,
// so an opacity dim on them is the right tool and out of scope here.
//
// Each entry was verified by reading the component's markup (not guessed
// from the class name) during the 2026-09 opacity survey — see
// `scripts/check-a11y-contrast/README.md` § "Add a new opacity exemption"
// for how to add one and what evidence it needs.
type opacityDecorativeEntry struct {
	fileSuffix string
	selector   string
	why        string
}

func (e opacityDecorativeEntry) matches(rule Rule) bool {
	return strings.HasSuffix(rule.File, e.fileSuffix) && rule.Selector == e.selector
}

// key identifies one entry in the `used` set. The `why` text is deliberately
// out: an entry reworded (or a second entry added for the same element under a
// clearer reason) is the same exemption.
func (e opacityDecorativeEntry) key() string {
	return e.fileSuffix + "|" + e.selector
}

var opacityDecorativeAllowlist = []opacityDecorativeEntry{
	{"StatusGlyph.svelte", ".status-glyph", "wraps an <Icon>, renders no text glyph"},
	{"VolumeBreadcrumb.svelte", ".read-only-indicator", `wraps <Icon name="lock">`},
	{"VolumeChooserMenu.svelte", ".read-only-indicator", `wraps <Icon name="lock">`},
	{"ConnectionDot.svelte", ".smb-indicator", "empty span, pure CSS-colored status dot"},
	{"ConnectionDot.svelte", ".smb-indicator-saved", "empty span, pure CSS-colored dot outline"},
	{"UsbSpeedDot.svelte", ".usb-speed-indicator", "empty span, pure CSS-colored status dot"},
	{"DriveIndexBadge.svelte", ".drive-index-badge", "empty <button>, pure CSS-colored status dot"},
	{"ImageIndexDriveBadge.svelte", ".image-index-drive-badge", `empty <span role="img">, pure CSS-colored status dot`},
	{"TabBar.svelte", ".warning-icon", "wraps an <Icon>"},
	{"TabBar.svelte", ".pin-icon", "wraps an <Icon>"},
	{"AiLocalSection.svelte", ".ram-projected", "empty bar-chart segment / legend swatch"},
	{"AiLocalSection.svelte", ".ram-freed", "empty bar-chart segment / legend swatch"},
	{"AskCmdrCostFooter.svelte", ".dot", `aria-hidden "·" divider, no informational content`},
	{"RepoChip.svelte", ".sep", `aria-hidden "·" divider, no informational content`},
	{"IndexingStatusBody.svelte", ".step-pending .step-marker", "wraps a <Spinner>/<Icon>, aria-hidden"},
	{"FileIcon.svelte", ".icon-wrapper.is-dimmed", `holds an alt="" <img> plus badge glyphs, no text; the row's name carries the meaning`},
	{"ScanPhaseBody.svelte", ".scan-throughput-sep", `aria-hidden "·" divider, no informational content`},
	{"DeleteDialog.svelte", ".scan-throughput-sep", `aria-hidden "·" divider, no informational content`},
	{"NewFolderDialog.svelte", ".suggestion-pending", `aria-hidden pulsing "…" placeholder, no informational content`},
}

func opacityDecorativeReason(rule Rule) (string, bool) {
	for _, e := range opacityDecorativeAllowlist {
		if e.matches(rule) {
			return e.why, true
		}
	}
	return "", false
}

// opacityModeledElsewhere lists (file-suffix, selector) pairs whose real
// composited contrast is already covered by a scenario synthesizer (a
// `FgExpr` in `dropdown_states.go` / `query_dialog_states.go` that models the
// same `opacity: N` as a `color-mix(..., transparent (1-N)%)` term). Empty
// today: `ToggleGroup.svelte`'s `.tg-hint` dropped its `opacity: 0.7` crutch
// after it failed AA (see the comment at `ToggleGroup.svelte` ~line 329) and
// nothing has re-added a modeled case since. Add an entry here when a
// synthesizer picks up a real `opacity` rule, so this check doesn't
// double-report what the synthesizer already verifies.
var opacityModeledElsewhere []opacityDecorativeEntry

func opacityIsModeledElsewhere(rule Rule) bool {
	for _, e := range opacityModeledElsewhere {
		if e.matches(rule) {
			return true
		}
	}
	return false
}

// opacityExemptionList names which of the two per-element lists an entry came
// from, for the stale report's message. Both are hand-maintained Go literals
// with the same (file-suffix, selector) shape, but they're removed for
// different reasons: a decorative entry goes when the element stops being
// decorative or stops existing; a modeled one goes when its synthesizer drops
// the term.
type opacityExemptionList int

const (
	decorativeExemptions opacityExemptionList = iota
	modeledExemptions
)

func (l opacityExemptionList) String() string {
	if l == modeledExemptions {
		return "opacityModeledElsewhere"
	}
	return "opacityDecorativeAllowlist"
}

// StaleOpacityExemption is an entry that excused nothing during a full run:
// nowhere under `apps/desktop/src` is there a rule with its (file-suffix,
// selector) that dims via `opacity`. Either the component was renamed or
// extracted, the class was renamed, or the dimming is gone.
//
// This is a hard failure rather than an advisory line, unlike the opacity
// findings themselves. A stale entry is a fact the tool can prove on its own
// (the `why` text describes an element the run never saw), and it fails SILENTLY
// otherwise: the entry just stops matching, and the element it covered comes
// back as a plain finding with nothing pointing at the hand-verified reason it
// used to carry. That happened to four entries at once when `ConnectionDot`,
// `UsbSpeedDot`, and `VolumeChooserMenu` were extracted out of
// `VolumeBreadcrumb.svelte`, and it read as four new a11y regressions.
type StaleOpacityExemption struct {
	List  opacityExemptionList
	Entry opacityDecorativeEntry
}

// noteOpacityExemptionUse marks every entry matching this rule as still doing
// work. Called for each rule that actually dims (0 < opacity < 1) and BEFORE
// the inactive/dragging skips, so an element that also picked up a `:disabled`
// or `.is-dragging` marker keeps its entry alive: that entry is redundant
// today, and load-bearing again the moment the other marker goes away, so
// calling it stale would walk someone into deleting a real exemption. A rule
// that no longer dims is not a use — the entry has nothing left to excuse.
func (a *Analyzer) noteOpacityExemptionUse(rule Rule) {
	if a.opacityExemptionsUsed == nil {
		a.opacityExemptionsUsed = make(map[string]bool)
	}
	for _, e := range opacityDecorativeAllowlist {
		if e.matches(rule) {
			a.opacityExemptionsUsed[e.key()] = true
		}
	}
	for _, e := range opacityModeledElsewhere {
		if e.matches(rule) {
			a.opacityExemptionsUsed[e.key()] = true
		}
	}
}

// StaleOpacityExemptions returns the entries no dimmed rule matched, in list
// then declaration order. Only meaningful after a full walk of
// `apps/desktop/src` (see main()): one file's worth of rules matches at most a
// couple of entries, so a partial run would call almost everything stale.
func (a *Analyzer) StaleOpacityExemptions() []StaleOpacityExemption {
	var stale []StaleOpacityExemption
	lists := []struct {
		kind    opacityExemptionList
		entries []opacityDecorativeEntry
	}{
		{decorativeExemptions, opacityDecorativeAllowlist},
		{modeledExemptions, opacityModeledElsewhere},
	}
	for _, l := range lists {
		for _, e := range l.entries {
			if !a.opacityExemptionsUsed[e.key()] {
				stale = append(stale, StaleOpacityExemption{List: l.kind, Entry: e})
			}
		}
	}
	return stale
}

// AnalyzeOpacity walks every parsed rule for a static `opacity: N < 1` that
// the WCAG/APCA rule walker can't fold into its color/background pairing. A
// rule is reported unless it's a disabled/inactive-component state (WCAG
// 1.4.3's own exemption), a hand-verified non-text/decorative element, or
// already covered by a scenario synthesizer. Deduplicates by (file,
// selector): a `prefers-reduced-motion` or other at-rule override of the
// same class state is one physical component, not a second finding.
func (a *Analyzer) AnalyzeOpacity(pf *ParsedFile) []OpacityFinding {
	var out []OpacityFinding
	seen := make(map[string]bool)

	for _, rule := range pf.Rules {
		if !rule.HasOpacity || rule.Opacity >= 1 {
			continue
		}
		// `opacity: 0` renders nothing: there's no visible glyph to have a
		// contrast ratio against anything. This is the CSS idiom for
		// "hidden until :hover/:focus/a state class reveals it" (a close
		// button, a hover-only overlay action, a singleton tooltip element
		// before it's positioned and shown), not a dimmed-but-readable text
		// case. The revealed state is a separate rule (its own selector,
		// commonly `:hover`/`:focus`/`.is-open`) and gets evaluated on its
		// own merits like any other rule.
		if rule.Opacity == 0 {
			continue
		}
		// Before any skip below: an entry that matches a real dim is doing
		// work even when another exemption would have caught the rule first.
		// See `noteOpacityExemptionUse`.
		a.noteOpacityExemptionUse(rule)
		if opacityInactiveSelector(rule) {
			continue
		}
		if opacityIsDraggingFeedback(rule) {
			continue
		}
		if _, ok := opacityDecorativeReason(rule); ok {
			continue
		}
		if opacityIsModeledElsewhere(rule) {
			continue
		}
		key := rule.File + "|" + rule.Selector
		if seen[key] {
			continue
		}
		seen[key] = true
		out = append(out, OpacityFinding{
			File:     rule.File,
			Line:     rule.Line,
			Selector: rule.Selector,
			Opacity:  rule.Opacity,
		})
	}
	return out
}
