package checks

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path"
	"path/filepath"
	"slices"
	"sort"
	"strings"
)

// vendoredCreditsPath is the hand-kept list of third-party material Cmdr ships
// that no lockfile knows about: an icon set, a font, a code snippet copied in.
// `third-party-notices` validates it and merges it into both generated files,
// so the Acknowledgements dialog and THIRD-PARTY-NOTICES.md credit it the way
// they credit a dependency. Repo-relative and slash-separated, like a glob in
// `Inputs`.
const vendoredCreditsPath = "scripts/check/checks/third-party-vendored.json"

// vendoredCredit is one entry of the vendored list. It asks for more than a
// lockfile package carries, because the licenses involved do: CC BY 4.0 wants
// the author, a link to the license, and whether the material was changed, and
// Apache 2.0 wants a changed file marked as changed. The dialog's row shows the
// package-shaped part (`asPackage`); the notices file carries all of it.
type vendoredCredit struct {
	Name string `json:"name"`
	// Version is optional: an icon copied out of a set rarely has one.
	Version    string `json:"version,omitempty"`
	Author     string `json:"author"`
	License    string `json:"license"`
	LicenseURL string `json:"license_url"`
	// URL is where the material comes from.
	URL string `json:"url"`
	// Changes says what Cmdr changed, or that it changed nothing. Required either
	// way, so "unchanged" is a statement rather than a forgotten field.
	Changes string `json:"changes"`
	// Files are the repo-relative files the credit covers. The check fails when
	// one no longer exists, so a credit can't outlive what it credits.
	Files []string `json:"files"`
}

// vendoredLicenses are the SPDX ids a vendored credit may carry. The point is
// catching the spelling a README uses (`CC BY 4.0`, `Apache 2`), which no SPDX
// tooling reads; add an id here when material under a new license ships.
var vendoredLicenses = []string{"Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "CC-BY-4.0", "CC0-1.0", "ISC", "MIT", "OFL-1.1"}

// loadVendoredCredits reads, sorts, and validates the vendored list.
func loadVendoredCredits(rootDir string) ([]vendoredCredit, error) {
	raw, err := os.ReadFile(filepath.Join(rootDir, filepath.FromSlash(vendoredCreditsPath)))
	if err != nil {
		return nil, fmt.Errorf("couldn't read %s: %w", vendoredCreditsPath, err)
	}
	credits, err := parseVendoredCredits(raw)
	if err != nil {
		return nil, err
	}
	if err := validateVendoredCredits(credits, rootDir); err != nil {
		return nil, err
	}
	return credits, nil
}

// parseVendoredCredits decodes the list and sorts it by name, so the generated
// files don't depend on the order it's kept in. An unknown field fails: a
// misspelled `licence_url` would otherwise decode as an empty field, and the
// failure would report a missing link where the problem is a typo.
func parseVendoredCredits(raw []byte) ([]vendoredCredit, error) {
	var parsed struct {
		Comment string           `json:"comment"`
		Credits []vendoredCredit `json:"credits"`
	}
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&parsed); err != nil {
		return nil, fmt.Errorf("couldn't parse %s: %w", vendoredCreditsPath, err)
	}
	sort.SliceStable(parsed.Credits, func(i, j int) bool {
		return strings.ToLower(parsed.Credits[i].Name) < strings.ToLower(parsed.Credits[j].Name)
	})
	return parsed.Credits, nil
}

// validateVendoredCredits reports every problem in the list at once, so one run
// names everything to fix.
func validateVendoredCredits(credits []vendoredCredit, rootDir string) error {
	var problems []string
	seen := map[string]bool{}
	for index, credit := range credits {
		label := credit.Name
		if label == "" {
			label = fmt.Sprintf("credit #%d", index+1)
		}
		if credit.Name != "" {
			key := strings.ToLower(credit.Name)
			if seen[key] {
				problems = append(problems, label+": another credit has the same name")
			}
			seen[key] = true
		}
		for _, problem := range creditProblems(credit, rootDir) {
			problems = append(problems, label+": "+problem)
		}
	}
	if len(problems) == 0 {
		return nil
	}
	return fmt.Errorf("%s has %d problem(s):\n%s", vendoredCreditsPath, len(problems), indentOutput(strings.Join(problems, "\n")))
}

// creditProblems is what's wrong with one credit on its own terms.
func creditProblems(credit vendoredCredit, rootDir string) []string {
	var problems []string
	for _, field := range []struct{ name, value string }{
		{"name", credit.Name},
		{"author", credit.Author},
		{"license", credit.License},
		{"license_url", credit.LicenseURL},
		{"url", credit.URL},
		{"changes", credit.Changes},
	} {
		if strings.TrimSpace(field.value) == "" {
			problems = append(problems, fmt.Sprintf("`%s` is empty", field.name))
		}
	}
	if credit.License != "" && !slices.Contains(vendoredLicenses, credit.License) {
		problems = append(problems, fmt.Sprintf("`%s` isn't a known SPDX id (%s)", credit.License, strings.Join(vendoredLicenses, ", ")))
	}
	if len(credit.Files) == 0 {
		problems = append(problems, "`files` is empty, so nothing says what the credit covers")
	}
	for _, file := range credit.Files {
		if problem := creditedFileProblem(file, rootDir); problem != "" {
			problems = append(problems, problem)
		}
	}
	return problems
}

// creditedFileProblem says why a credited path doesn't hold up, or returns "".
func creditedFileProblem(file, rootDir string) string {
	if file != path.Clean(file) || !filepath.IsLocal(filepath.FromSlash(file)) {
		return fmt.Sprintf("`%s` isn't a clean repo-relative path", file)
	}
	info, err := os.Stat(filepath.Join(rootDir, filepath.FromSlash(file)))
	if err != nil || !info.Mode().IsRegular() {
		return fmt.Sprintf("`%s` isn't a file in the repo (renamed or deleted?)", file)
	}
	return ""
}

// asPackage is the credit in the shape the dialog renders every list with.
func (credit vendoredCredit) asPackage() attributedPackage {
	return attributedPackage{Name: credit.Name, Version: credit.Version, License: credit.License, URL: credit.URL}
}

// writeVendoredSection credits each vendored work with everything its license
// asks an attribution to carry, more than the dialog's row has room for: the
// author, the source, a link to the license, what Cmdr changed, and the files.
func writeVendoredSection(b *strings.Builder, credits []vendoredCredit) {
	b.WriteString("## Icons and other files\n\n")
	for _, credit := range credits {
		fmt.Fprintf(b, "- **%s**", credit.Name)
		if credit.Version != "" {
			fmt.Fprintf(b, " %s", credit.Version)
		}
		fmt.Fprintf(b, " by %s, %s, <%s>\n", credit.Author, credit.License, credit.URL)
		fmt.Fprintf(b, "  - License: <%s>\n", credit.LicenseURL)
		fmt.Fprintf(b, "  - Changes: %s\n", credit.Changes)
		fmt.Fprintf(b, "  - Files: `%s`\n", strings.Join(credit.Files, "`, `"))
	}
	b.WriteString("\n")
}
