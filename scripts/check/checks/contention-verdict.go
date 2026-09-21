package checks

import (
	"os"
	"os/exec"
	"runtime"
	"strconv"
	"strings"
)

// The vocabulary a test lane uses to say what an isolated re-run concluded about a
// red run, shared by every lane that has one: the Rust suites
// (`rust-test-contention.go`) and the macOS Playwright lane (`e2e-contention.go`).
//
// Both lanes face the same problem from opposite directions. The Rust suite starves
// its own tests with 200 threads on 16 cores; the E2E lane starves its specs with
// three Tauri instances, three Playwright processes, and three video recorders, on a
// machine somebody is also working on. Both answer it the same way: re-run the
// failures ALONE and let the outcome classify them, rather than guessing from load.
//
// Load is never the gate. The isolated re-run is. Load enters at exactly one point:
// the "needed headroom" verdict is the only one whose meaning depends on the machine
// being quiet, so a re-run that itself ran hot demotes it to inconclusive. A test
// that passes alone despite load is still contention, and one that fails alone with
// headroom is still broken; neither conclusion needs a threshold.
//
// Keeping this in one place is the contract: a lane may say WHERE a re-run happens
// and what "alone" means for it, never what a red run means.

// ContentionVerdict is what the isolated re-runs concluded about one failing test.
type ContentionVerdict string

const (
	// VerdictContention: passed alone at the same deadline. The suite starved it.
	VerdictContention ContentionVerdict = "contention"
	// VerdictTooSlow: needed headroom even alone, on a quiet machine. Wants tweaking or
	// an explicit, documented per-test override, not silent absorption.
	VerdictTooSlow ContentionVerdict = "too-slow"
	// VerdictInconclusive: needed headroom, but the re-run itself ran on a busy machine,
	// so "it got slower" can't be told from "it was starved again". Reported, never
	// dressed up as either.
	VerdictInconclusive ContentionVerdict = "inconclusive"
	// VerdictReal: failed even alone with headroom.
	VerdictReal ContentionVerdict = "real"
)

// BusyLoadPerCore is where a machine is considered too busy for the "needed headroom"
// verdict to mean anything. Normal interactive work sits well under 1 runnable thread
// per core; the saturated runs this exists for measured ~12 per core.
const BusyLoadPerCore = 1.5

// WarnOnly decides whether a red run may be softened to a warn. Contention is proven
// harmless, and inconclusive means the machine was too busy to prove anything, so
// failing on it would just punish the user for running the suite while busy: exactly
// the case this whole mechanism exists to stop mislabelling. A too-slow or real
// verdict keeps the run red.
func WarnOnly(verdicts []ContentionVerdict) bool {
	if len(verdicts) == 0 {
		return false
	}
	for _, v := range verdicts {
		if v != VerdictContention && v != VerdictInconclusive {
			return false
		}
	}
	return true
}

// LoadSampler reports the current load average per core. Injected for testability.
type LoadSampler func() float64

// LoadPerCore is the 1-minute load average divided by the core count, the shape the
// busy/quiet judgement is expressed in.
func LoadPerCore() float64 {
	cores := runtime.NumCPU()
	if cores <= 0 {
		return 0
	}
	return LoadAverage() / float64(cores)
}

// LoadAverage returns the 1-minute load average, or 0 when it can't be read. An
// unreadable load reads as quiet, which keeps a run red rather than softening it.
func LoadAverage() float64 {
	if raw, err := os.ReadFile("/proc/loadavg"); err == nil { // Linux
		if fields := strings.Fields(string(raw)); len(fields) > 0 {
			if v, err := strconv.ParseFloat(fields[0], 64); err == nil {
				return v
			}
		}
	}
	out, err := exec.Command("sysctl", "-n", "vm.loadavg").Output() // macOS: "{ 1.83 2.05 2.11 }"
	if err != nil {
		return 0
	}
	fields := strings.Fields(strings.Trim(strings.TrimSpace(string(out)), "{}"))
	if len(fields) == 0 {
		return 0
	}
	v, err := strconv.ParseFloat(fields[0], 64)
	if err != nil {
		return 0
	}
	return v
}
