package checks

import "testing"

func TestE2EWaitScaleStaysAtOneOnAQuietMachine(t *testing.T) {
	for _, ambient := range []float64{0, 0.001, -1} {
		if got := e2eWaitScale(ambient); got != 1 {
			t.Errorf("ambient load %.3f/core: got scale %.2f, want 1 (today's budgets are tuned for a quiet box)", ambient, got)
		}
	}
}

func TestE2EWaitScaleTracksAmbientLoad(t *testing.T) {
	cases := []struct {
		ambient float64
		want    float64
	}{
		{0.5, 1.5}, // half a runnable thread per core of somebody else's work
		{1.0, 2.0}, // one: every test thread is sharing its core
		{2.0, 3.0},
	}
	for _, c := range cases {
		if got := e2eWaitScale(c.ambient); got != c.want {
			t.Errorf("ambient load %.1f/core: got scale %.2f, want %.2f", c.ambient, got, c.want)
		}
	}
}

func TestE2EWaitScaleClampsToTheHeadroomFactor(t *testing.T) {
	// Past the isolation re-run's own escalation, more patience is no longer cheaper
	// than re-running the spec alone, and a wedged app would just sit there.
	for _, ambient := range []float64{3.0, 12.0, 198.0} {
		if got := e2eWaitScale(ambient); got != e2eHeadroomFactor {
			t.Errorf("ambient load %.1f/core: got scale %.2f, want the %dx clamp", ambient, got, e2eHeadroomFactor)
		}
	}
}

func TestE2EWaitScaleRoundsToOneDecimal(t *testing.T) {
	// The value is rendered into an env var and printed by `global-setup.ts`; an
	// unrounded float would read as noise ("1.3333333333333333x").
	if got := e2eWaitScale(0.33333); got != 1.3 {
		t.Errorf("got %v, want 1.3", got)
	}
}
