//! When one listing refreshes: batches wait out the cooldown, hourglass flips don't.
//!
//! Two kinds of work reach a listing. Index batches say sizes moved; they merge and run at most once
//! per [`COOLDOWN`], with a trailing run so the last one always lands. Rechecks say a row's hourglass
//! flips on its own at a known moment (an update crossing the two-second mark, or a shown hourglass
//! ending its minimum time on screen), with no write to announce it. They run at that moment,
//! outside the cooldown: waiting it out would show the hourglass up to two seconds late, or skip a
//! short one the webview already read straight from the index.

use std::time::Duration;

use tokio::time::Instant;

use super::Touched;

/// The shortest gap between two batch refreshes of one listing. A busy disk moves a pane on `~`
/// every second; a size that settles two seconds late reads the same, and half the refreshes cost
/// half.
pub(super) const COOLDOWN: Duration = Duration::from_secs(2);

/// One listing's waiting work.
#[derive(Default)]
pub(super) struct Schedule {
    /// What the batches since the last refresh touched.
    pending: Option<Touched>,
    /// When the last batch refresh ran, for the cooldown.
    last_refresh: Option<Instant>,
    /// Rows to re-read when their hourglass flips, and the earliest flip.
    recheck: Option<(Instant, Touched)>,
}

/// The work due now: batch rows, and rows whose hourglass flips.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Due {
    pub(super) batch: Option<Touched>,
    pub(super) recheck: Option<Touched>,
}

impl Schedule {
    /// Folds an index batch in.
    pub(super) fn add_batch(&mut self, touched: Touched) {
        merge_into(&mut self.pending, touched);
    }

    /// Asks for `rows` to be re-read `after` from `now`. Several asks keep the earliest moment and
    /// every row: a row read before its own flip reports a fresh `after`, and asks again.
    pub(super) fn recheck(&mut self, now: Instant, after: Duration, rows: Touched) {
        let at = now + after;
        match &mut self.recheck {
            Some((due, waiting)) => {
                *due = (*due).min(at);
                waiting.merge(rows);
            }
            None => self.recheck = Some((at, rows)),
        }
    }

    /// Takes whatever is due at `now`.
    pub(super) fn take_due(&mut self, now: Instant) -> Due {
        let batch_due = self.pending.is_some() && self.last_refresh.is_none_or(|at| now >= at + COOLDOWN);
        let batch = if batch_due {
            self.last_refresh = Some(now);
            self.pending.take()
        } else {
            None
        };
        let recheck = match self.recheck.take() {
            Some((due, rows)) if due <= now => Some(rows),
            waiting => {
                self.recheck = waiting;
                None
            }
        };
        Due { batch, recheck }
    }

    /// When this listing next has work due, if it has any waiting.
    pub(super) fn next_due(&self) -> Option<Instant> {
        let batch = self
            .pending
            .as_ref()
            .map(|_| self.last_refresh.map_or_else(Instant::now, |at| at + COOLDOWN));
        let recheck = self.recheck.as_ref().map(|(due, _)| *due);
        batch.into_iter().chain(recheck).min()
    }
}

fn merge_into(slot: &mut Option<Touched>, touched: Touched) {
    match slot {
        Some(waiting) => waiting.merge(touched),
        None => *slot = Some(touched),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn rows(children: &[&str]) -> Touched {
        Touched::Rows {
            own: false,
            children: children.iter().map(|c| c.to_string()).collect::<BTreeSet<_>>(),
        }
    }

    #[test]
    fn a_batch_inside_the_cooldown_waits_for_it() {
        let now = Instant::now();
        let mut schedule = Schedule::default();
        schedule.add_batch(rows(&["Library"]));
        assert_eq!(schedule.take_due(now).batch, Some(rows(&["Library"])));

        schedule.add_batch(rows(&["Library"]));
        assert_eq!(schedule.take_due(now + Duration::from_secs(1)), Due::default());
        assert_eq!(schedule.next_due(), Some(now + COOLDOWN), "the trailing refresh");
        assert_eq!(schedule.take_due(now + COOLDOWN).batch, Some(rows(&["Library"])));
    }

    #[test]
    fn a_recheck_runs_at_its_moment_even_inside_the_cooldown() {
        let now = Instant::now();
        let mut schedule = Schedule::default();
        schedule.add_batch(rows(&["Library"]));
        let _ = schedule.take_due(now);

        // The batch refresh read `Library` 1.2 s into an update: it shows 0.8 s from now.
        schedule.recheck(now, Duration::from_millis(800), rows(&["Library"]));
        assert_eq!(schedule.next_due(), Some(now + Duration::from_millis(800)));
        assert_eq!(schedule.take_due(now + Duration::from_millis(500)), Due::default());
        assert_eq!(
            schedule.take_due(now + Duration::from_millis(800)),
            Due {
                batch: None,
                recheck: Some(rows(&["Library"])),
            }
        );
        assert_eq!(schedule.next_due(), None, "nothing left waiting");
    }

    #[test]
    fn rechecks_merge_at_the_earliest_moment() {
        let now = Instant::now();
        let mut schedule = Schedule::default();
        schedule.recheck(now, Duration::from_secs(1), rows(&["a"]));
        schedule.recheck(now, Duration::from_millis(300), rows(&["b"]));
        assert_eq!(
            schedule.take_due(now + Duration::from_millis(300)).recheck,
            Some(rows(&["a", "b"]))
        );
    }

    #[test]
    fn a_recheck_leaves_the_cooldown_alone() {
        // A recheck isn't a batch refresh, so a batch right after it still runs at once.
        let now = Instant::now();
        let mut schedule = Schedule::default();
        schedule.recheck(now, Duration::ZERO, rows(&["a"]));
        let _ = schedule.take_due(now);
        schedule.add_batch(rows(&["b"]));
        assert_eq!(schedule.take_due(now).batch, Some(rows(&["b"])));
    }
}
