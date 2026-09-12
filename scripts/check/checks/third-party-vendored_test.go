package checks

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// completeCredit is a vendored credit with every field filled in, covering one
// file it creates under root.
func completeCredit(t *testing.T, root string) vendoredCredit {
	t.Helper()
	path := "icons/drive.svg"
	full := filepath.Join(root, filepath.FromSlash(path))
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(full, []byte("<svg/>"), 0o644); err != nil {
		t.Fatal(err)
	}
	return vendoredCredit{
		Name:       "selfh.st icons",
		Author:     "selfh.st",
		License:    "CC-BY-4.0",
		LicenseURL: "https://github.com/selfhst/icons/blob/main/LICENSE",
		URL:        "https://github.com/selfhst/icons",
		Changes:    "None.",
		Files:      []string{path},
	}
}

func TestValidateVendoredCreditsAcceptsACompleteCredit(t *testing.T) {
	root := t.TempDir()
	if err := validateVendoredCredits([]vendoredCredit{completeCredit(t, root)}, root); err != nil {
		t.Errorf("a complete credit should validate, got: %v", err)
	}
}

func TestValidateVendoredCreditsNamesEveryMissingField(t *testing.T) {
	// CC BY 4.0 asks for the author, a link to the license, and whether the
	// material was changed. A credit missing one doesn't meet the license it
	// names, and one run should list everything to fill in.
	root := t.TempDir()
	credit := completeCredit(t, root)
	credit.Author, credit.LicenseURL, credit.Changes = "", "", ""

	err := validateVendoredCredits([]vendoredCredit{credit}, root)
	if err == nil {
		t.Fatal("expected a failure for a credit with empty required fields")
	}
	for _, field := range []string{"author", "license_url", "changes"} {
		if !strings.Contains(err.Error(), field) {
			t.Errorf("the failure should name the empty `%s`, got: %v", field, err)
		}
	}
}

func TestValidateVendoredCreditsRejectsACreditCoveringNoFiles(t *testing.T) {
	root := t.TempDir()
	credit := completeCredit(t, root)
	credit.Files = nil
	if err := validateVendoredCredits([]vendoredCredit{credit}, root); err == nil {
		t.Error("expected a failure for a credit that names no files")
	}
}

func TestValidateVendoredCreditsRejectsALicenseThatIsntAKnownSPDXID(t *testing.T) {
	// The spelling a person types from a README, which no SPDX tooling reads.
	root := t.TempDir()
	credit := completeCredit(t, root)
	credit.License = "CC BY 4.0"

	err := validateVendoredCredits([]vendoredCredit{credit}, root)
	if err == nil {
		t.Fatal("expected a failure for a license that isn't a known SPDX id")
	}
	if !strings.Contains(err.Error(), "CC BY 4.0") {
		t.Errorf("the failure should quote the license, got: %v", err)
	}
}

func TestValidateVendoredCreditsRejectsADuplicateName(t *testing.T) {
	// Names differing only in case would sort next to each other and read as one
	// credit listed twice.
	root := t.TempDir()
	first := completeCredit(t, root)
	second := completeCredit(t, root)
	second.Name = "Selfh.st Icons"

	err := validateVendoredCredits([]vendoredCredit{first, second}, root)
	if err == nil {
		t.Fatal("expected a failure for two credits with the same name")
	}
	if !strings.Contains(err.Error(), "Selfh.st Icons") {
		t.Errorf("the failure should name the duplicate, got: %v", err)
	}
}

func TestValidateVendoredCreditsRejectsAFileThatNoLongerExists(t *testing.T) {
	// The credit outlived the file: someone renamed or deleted an icon and the
	// list still claims to cover it.
	root := t.TempDir()
	credit := completeCredit(t, root)
	credit.Files = append(credit.Files, "icons/gone.svg")

	err := validateVendoredCredits([]vendoredCredit{credit}, root)
	if err == nil {
		t.Fatal("expected a failure for a credited file that doesn't exist")
	}
	if !strings.Contains(err.Error(), "icons/gone.svg") {
		t.Errorf("the failure should name the missing file, got: %v", err)
	}
}

func TestValidateVendoredCreditsRejectsAPathOutsideTheRepo(t *testing.T) {
	// A path that climbs out of the repo could exist on one machine and not on
	// the next, so it would verify nothing.
	root := t.TempDir()
	for _, path := range []string{"../elsewhere.svg", "/etc/hosts"} {
		credit := completeCredit(t, root)
		credit.Files = []string{path}
		if err := validateVendoredCredits([]vendoredCredit{credit}, root); err == nil {
			t.Errorf("expected a failure for the non-repo path %q", path)
		}
	}
}

func TestParseVendoredCreditsRejectsAnUnknownField(t *testing.T) {
	// A misspelled `licence_url` would otherwise decode as an empty field, and
	// the failure would say the link is missing when the real problem is a typo.
	_, err := parseVendoredCredits([]byte(`{"credits": [{"name": "x", "licence_url": "y"}]}`))
	if err == nil {
		t.Fatal("expected a failure for an unknown field")
	}
	if !strings.Contains(err.Error(), "licence_url") {
		t.Errorf("the failure should name the unknown field, got: %v", err)
	}
}

func TestParseVendoredCreditsSortsByName(t *testing.T) {
	// The generated files must be byte-stable whatever order the list is kept in.
	credits, err := parseVendoredCredits([]byte(`{"credits": [{"name": "Material Symbols"}, {"name": "material Design Icons"}]}`))
	if err != nil {
		t.Fatalf("unexpected failure: %v", err)
	}
	if len(credits) != 2 || credits[0].Name != "material Design Icons" {
		t.Errorf("expected a case-insensitive sort by name, got %+v", credits)
	}
}

func TestRenderPackagesJSONMergesVendoredCredits(t *testing.T) {
	// The dialog renders every list with the same row markup, so a credit reaches
	// it in exactly the shape a lockfile package does.
	credits := []vendoredCredit{{
		Name:       "Material Symbols",
		Author:     "Google",
		License:    "Apache-2.0",
		LicenseURL: "https://github.com/google/material-design-icons/blob/master/LICENSE",
		URL:        "https://github.com/google/material-design-icons",
		Changes:    "Recolored.",
		Files:      []string{"a.svg"},
	}}
	out, err := renderPackagesJSON(nil, nil, credits)
	if err != nil {
		t.Fatalf("unexpected failure: %v", err)
	}

	var parsed struct {
		Vendored []map[string]string `json:"vendored"`
	}
	if err := json.Unmarshal(out, &parsed); err != nil {
		t.Fatalf("the output isn't JSON: %v", err)
	}
	want := map[string]string{
		"name":    "Material Symbols",
		"version": "",
		"license": "Apache-2.0",
		"url":     "https://github.com/google/material-design-icons",
	}
	if len(parsed.Vendored) != 1 || len(parsed.Vendored[0]) != len(want) {
		t.Fatalf("expected one credit shaped like a package, got %v", parsed.Vendored)
	}
	for key, value := range want {
		if parsed.Vendored[0][key] != value {
			t.Errorf("%s: got %q, want %q", key, parsed.Vendored[0][key], value)
		}
	}
}

func TestRenderPackagesJSONWritesAnEmptyVendoredListAsAnArray(t *testing.T) {
	// The dialog iterates the list, and `null` would throw.
	out, err := renderPackagesJSON(nil, nil, nil)
	if err != nil {
		t.Fatalf("unexpected failure: %v", err)
	}
	if !strings.Contains(string(out), `"vendored": []`) {
		t.Errorf("expected an empty array, got:\n%s", out)
	}
}

func TestRenderNoticesCreditsVendoredMaterialInFull(t *testing.T) {
	// The notices file is where a credit meets CC BY 4.0 and Apache 2.0 in full:
	// the author, the license and a link to it, the source, and what changed.
	credit := vendoredCredit{
		Name:       "Material Symbols",
		Author:     "Google",
		License:    "Apache-2.0",
		LicenseURL: "https://github.com/google/material-design-icons/blob/master/LICENSE",
		URL:        "https://github.com/google/material-design-icons",
		Changes:    "Recolored to the Android green.",
		Files:      []string{"icons/a.svg", "icons/b.svg"},
	}
	out := string(renderNotices(rustCollection{}, nil, []vendoredCredit{credit}))

	for _, needle := range []string{
		"- Icons and other files: 1",
		"**Material Symbols** by Google, Apache-2.0, <https://github.com/google/material-design-icons>",
		"License: <https://github.com/google/material-design-icons/blob/master/LICENSE>",
		"Changes: Recolored to the Android green.",
		"Files: `icons/a.svg`, `icons/b.svg`",
	} {
		if !strings.Contains(out, needle) {
			t.Errorf("rendered notices missing %q, got:\n%s", needle, out)
		}
	}
}

func TestVendoredCreditsCoverOnlyFilesTheNoticesCheckFingerprints(t *testing.T) {
	// The runner skips a check whose inputs didn't change, so a credited file
	// outside the check's `Inputs` could be deleted or renamed while the check
	// keeps answering green from cache. Credit a file somewhere new, and this
	// names the glob to add.
	root := repoRootForTest(t)
	raw, err := os.ReadFile(filepath.Join(root, filepath.FromSlash(vendoredCreditsPath)))
	if err != nil {
		t.Fatalf("reading the vendored credits: %v", err)
	}
	credits, err := parseVendoredCredits(raw)
	if err != nil {
		t.Fatalf("parsing the vendored credits: %v", err)
	}
	if len(credits) == 0 {
		t.Fatal("found no vendored credits; the list or the parse is broken")
	}

	def := GetCheckByID("desktop-third-party-notices")
	if def == nil {
		t.Fatal("no `desktop-third-party-notices` check in the registry")
	}
	data, err := CollectRepoFingerprintData(root)
	if err != nil {
		t.Fatalf("CollectRepoFingerprintData: %v", err)
	}
	patterns := data.PatternsFor(def)

	if !matchesAny(vendoredCreditsPath, patterns) {
		t.Errorf("`desktop-third-party-notices` doesn't fingerprint %s itself", vendoredCreditsPath)
	}
	for _, credit := range credits {
		for _, file := range credit.Files {
			if !matchesAny(file, patterns) {
				t.Errorf("%q credits %s, which `desktop-third-party-notices` doesn't fingerprint: add a glob for it to the check's `Inputs`",
					credit.Name, file)
			}
		}
	}
}
