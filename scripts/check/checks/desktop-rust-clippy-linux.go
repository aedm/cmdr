package checks

import (
	"fmt"
	"regexp"
	"strings"
)

// RunClippyLinux lints the workspace for the Linux target, from a Mac, with the command
// CI's "Desktop (Rust)" job runs natively on ubuntu. The host `clippy` lane only ever
// sees macOS's `cfg`s, so a lint in a `#[cfg(target_os = "linux")]` module (all of
// `volumes_linux/`, the GTK paths) reached `main` unlinted and turned CI red.
//
// It shares the tests lane's image and target volume (`desktop-rust-linux-container.go`):
// clippy's units land in their own fingerprints, so the two don't thrash each other, and
// the dependency crates both need come from one warm cache. It never auto-fixes: the
// host `clippy` lane (which this one depends on) already ran `--fix` for everything the
// two targets share, and what's left is Linux-only code the author should see.
func RunClippyLinux(ctx *CheckContext) (CheckResult, error) {
	if skip, unavailable := dockerUnavailable(); unavailable {
		return skip, nil
	}

	selection, err := linuxSelectionArgs(ctx.RootDir)
	if err != nil {
		return CheckResult{}, err
	}

	env, err := prepareLinuxContainer(ctx.RootDir)
	if err != nil {
		return CheckResult{}, err
	}

	container := linuxContainerName("clippy")
	if err := startLinuxContainer(container, ctx.RootDir, env); err != nil {
		return CheckResult{}, err
	}
	defer removeLinuxContainer(container)

	output, err := dockerExec(container, containerCargoScript(linuxClippyArgs(selection)...))
	if err != nil {
		return CheckResult{}, fmt.Errorf("clippy found issues on Linux%s\n%s",
			env.buildNote(), indentOutput(trimCargoProgress(output)))
	}
	result := clippySuccess(output, " on Linux")
	result.Message += env.buildNote()
	return result, nil
}

// linuxClippyArgs is CI's clippy invocation (`desktop-rust-clippy` under `--ci`) with the
// Linux selection. Pure, so the parity with CI is testable without a container.
func linuxClippyArgs(selection []string) []string {
	args := append([]string{"clippy", "--locked", "--all-targets"}, selection...)
	return append(args, "--", "-D", "warnings")
}

// cargoProgressRE matches cargo's per-crate progress and housekeeping lines, which say
// nothing about a failure. Anchored to cargo's own right-aligned status column, so a
// diagnostic that merely quotes one of these words is kept.
var cargoProgressRE = regexp.MustCompile(
	`^\s*(?:Compiling|Checking|Fresh|Downloading|Downloaded|Updating|Locking|Adding|Blocking|Finished) \S`,
)

// trimCargoProgress drops cargo's progress lines from a failed build, leaving the
// diagnostics (each with its `--> file:line:col`) and the `could not compile` summary.
// Per-line and structural: it can only ever keep too much, never drop a diagnostic.
func trimCargoProgress(output string) string {
	lines := strings.Split(StripANSI(output), "\n")
	kept := make([]string, 0, len(lines))
	for _, line := range lines {
		if cargoProgressRE.MatchString(line) {
			continue
		}
		kept = append(kept, line)
	}
	return strings.TrimSpace(strings.Join(kept, "\n"))
}
