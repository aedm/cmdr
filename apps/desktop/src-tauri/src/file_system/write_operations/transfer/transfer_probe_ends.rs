//! What the probe asks a transfer's two volumes about their connections, and
//! the receive rate it derives from the answers.
//!
//! Apart from the live table in `transfer_probe.rs` because neither half needs
//! it: [`TransferEnds`] is two `Arc`s and two questions, and [`InboundWindow`] is
//! a pure function of the readings fed to it, so the rate arithmetic is tested
//! without a watchdog around it.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use crate::file_system::volume::{ConnectionLiveness, Volume};

/// How many once-a-second readings the receive rate spans: the newest and the
/// one four ticks before it.
///
/// TCP hands a slow response over in bursts (roughly 64 KB at a time), so a rate
/// over a single tick swings between zero and a burst. Four seconds smooths a
/// ~20 KB/s trickle into one steady number while still following a link that
/// speeds up or stalls within a few seconds.
const INBOUND_RATE_SAMPLES: usize = 5;

/// A transfer's two volumes, held ONLY to ask their connections two things:
/// whether either has been PROVEN dead (the gate on the watchdog's one
/// aggressive act) and how many bytes the SOURCE has received.
///
/// Named ends rather than a list because the questions differ: death on either
/// end matters, while the receive rate is the source's alone. A download's bytes
/// arrive on the source's connection; an upload's destination only ever sends
/// acknowledgements back, which aren't the file.
pub(in crate::file_system::write_operations::transfer) struct TransferEnds {
    source: Option<Arc<dyn Volume>>,
    destination: Option<Arc<dyn Volume>>,
}

impl TransferEnds {
    pub(in crate::file_system::write_operations::transfer) fn new(
        source: Arc<dyn Volume>,
        destination: Arc<dyn Volume>,
    ) -> Self {
        Self {
            source: Some(source),
            destination: Some(destination),
        }
    }

    /// Neither end: nobody to ask, so no verdict and no rate.
    #[cfg(test)]
    pub(in crate::file_system::write_operations::transfer) fn none() -> Self {
        Self {
            source: None,
            destination: None,
        }
    }

    /// A source alone, for the suites that script one connection.
    #[cfg(test)]
    pub(in crate::file_system::write_operations::transfer) fn source_only(source: Arc<dyn Volume>) -> Self {
        Self {
            source: Some(source),
            destination: None,
        }
    }

    /// Has either end's connection been proven dead?
    pub(super) fn any_proven_dead(&self) -> bool {
        [&self.source, &self.destination]
            .into_iter()
            .flatten()
            .any(|v| v.connection_liveness() == Some(ConnectionLiveness::Dead))
    }

    /// The source connection's running count of bytes received, if it keeps one.
    pub(super) fn source_bytes_received(&self) -> Option<u64> {
        self.source.as_ref().and_then(|v| v.connection_bytes_received())
    }
}

/// The last few readings of the source's received-bytes count, one per watchdog
/// tick, and the rate across them.
// DEFAULT-OK: an empty window claims nothing about the disk; it's "no readings yet", which is also what it answers.
#[derive(Default)]
pub(super) struct InboundWindow {
    samples: VecDeque<(Duration, u64)>,
}

impl InboundWindow {
    /// Take this tick's reading. A missing one, or one BELOW the last (a rebuilt
    /// session counts from zero again), starts the window over: a delta across
    /// either would be a number about no connection in particular.
    pub(super) fn record(&mut self, now: Duration, reading: Option<u64>) {
        let Some(bytes) = reading else {
            self.samples.clear();
            return;
        };
        if self.samples.back().is_some_and(|&(_, last)| bytes < last) {
            self.samples.clear();
        }
        self.samples.push_back((now, bytes));
        while self.samples.len() > INBOUND_RATE_SAMPLES {
            self.samples.pop_front();
        }
    }

    /// Forget every reading, for the ticks the operation stands still on purpose.
    pub(super) fn clear(&mut self) {
        self.samples.clear();
    }

    /// Bytes per second across the window, or `None` until there are two
    /// readings to compare, and whenever nothing arrived between them.
    pub(super) fn bytes_per_second(&self) -> Option<u64> {
        let (&(t0, b0), &(t1, b1)) = (self.samples.front()?, self.samples.back()?);
        let elapsed_ms = u64::try_from(t1.saturating_sub(t0).as_millis()).unwrap_or(u64::MAX);
        let received = b1.saturating_sub(b0);
        if elapsed_ms == 0 || received == 0 {
            return None;
        }
        Some(received.saturating_mul(1_000) / elapsed_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    /// One reading is a count, not a rate.
    #[test]
    fn a_rate_needs_two_readings() {
        let mut window = InboundWindow::default();
        window.record(secs(1), Some(64_000));
        assert_eq!(window.bytes_per_second(), None);
        window.record(secs(2), Some(84_000));
        assert_eq!(window.bytes_per_second(), Some(20_000));
    }

    /// TCP delivers a trickle in bursts, so single ticks read 64 KB, 0, 0, 64 KB.
    /// The window turns that into the steady number a person can read.
    #[test]
    fn bursts_average_out_across_the_window() {
        let mut window = InboundWindow::default();
        for (tick, total) in [(1, 0), (2, 64_000), (3, 64_000), (4, 64_000), (5, 128_000)] {
            window.record(secs(tick), Some(total));
        }
        assert_eq!(window.bytes_per_second(), Some(32_000));

        // The oldest reading leaves as a new one arrives.
        window.record(secs(6), Some(128_000));
        assert_eq!(window.bytes_per_second(), Some(16_000));
    }

    /// Nothing arriving is no rate at all, rather than a rate of zero: "the
    /// source is sending 0 B/s" is a sentence nobody needs.
    #[test]
    fn nothing_arriving_is_no_rate() {
        let mut window = InboundWindow::default();
        window.record(secs(1), Some(5_000));
        window.record(secs(2), Some(5_000));
        assert_eq!(window.bytes_per_second(), None);
    }

    /// A rebuilt session starts counting from zero. A delta across the drop
    /// would describe no connection at all, so the window starts over.
    #[test]
    fn a_count_that_drops_starts_the_window_over() {
        let mut window = InboundWindow::default();
        window.record(secs(1), Some(900_000));
        window.record(secs(2), Some(950_000));
        window.record(secs(3), Some(1_000));
        assert_eq!(window.bytes_per_second(), None, "one reading since the drop");
        window.record(secs(4), Some(11_000));
        assert_eq!(window.bytes_per_second(), Some(10_000));
    }

    /// A source with no connection to count (local disk, MTP) never has a rate,
    /// and losing the count mid-transfer forgets the old readings.
    #[test]
    fn a_missing_reading_forgets_the_window() {
        let mut window = InboundWindow::default();
        window.record(secs(1), Some(0));
        window.record(secs(2), None);
        window.record(secs(3), Some(50_000));
        assert_eq!(window.bytes_per_second(), None);
    }
}
