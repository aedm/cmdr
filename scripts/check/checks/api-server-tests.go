package checks

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strconv"
)

// RunApiServerTests runs tests on the API server.
func RunApiServerTests(ctx *CheckContext) (CheckResult, error) {
	serverDir := filepath.Join(ctx.RootDir, "apps", "api-server")

	report, cleanup, reportErr := newVitestReportPath("api-server")
	defer cleanup()

	cmd := exec.Command("pnpm", "test")
	cmd.Dir = serverDir
	if reportErr == nil {
		cmd.Env = append(os.Environ(), "VITEST_JSON_REPORT="+report)
	}
	output, err := RunCommand(cmd, true)
	// Before the verdict branch, so a red run records WHICH tests went red.
	recordVitestTests(ctx, report, serverDir)
	if err != nil {
		return CheckResult{}, fmt.Errorf("tests failed: %s", diagnoseVitestFailure(report, serverDir, output))
	}

	// Extract test count
	re := regexp.MustCompile(`Tests\s+(\d+) passed`)
	matches := re.FindStringSubmatch(output)
	if len(matches) > 1 {
		count, _ := strconv.Atoi(matches[1])
		result := Success(fmt.Sprintf("%d %s passed", count, Pluralize(count, "test", "tests")))
		result.Total = count
		return result, nil
	}
	return Success("All tests passed"), nil
}
