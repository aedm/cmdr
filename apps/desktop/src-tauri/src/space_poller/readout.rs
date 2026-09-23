//! What a pane's disk-space readout shows, as numbers, so the poller emits only when it would change.
//!
//! The frontend renders free and total space as `formatFileSizeWithFormat` does (the friendliest unit,
//! two decimals, base 1024 or 1000 per `appearance.fileSizeFormat`), the status bar's free percent and
//! the usage bar's used percent as whole numbers, and the low-disk-space toast's free percent to one
//! decimal. [`DisplayedSpace`] is those figures at those resolutions, never the text: the webview owns
//! locale, separators, and unit labels. Two readings with equal `DisplayedSpace` draw the same pixels,
//! so emitting the second would repaint both status bars for nothing.
//!
//! ❗ Mirrors `src/lib/units/byte-size.ts` (`formatFileSizeWithFormat`) and
//! `src/lib/file-explorer/disk-space-utils.ts` (the percentages). Change the precision there, change
//! it here, or the readout goes stale by up to one step.

use serde::Deserialize;

use crate::file_system::volume::SpaceInfo;

/// The `appearance.fileSizeFormat` setting: which base the size units step by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum FileSizeFormat {
    /// Base 1024 (`KB`, `MB`, `GB`), the setting's default.
    #[default]
    Binary,
    /// Base 1000 (`kB`, `MB`, `GB`).
    Si,
}

impl FileSizeFormat {
    /// The setting's stored value, or the default for anything it doesn't recognize.
    pub fn from_setting(value: Option<&str>) -> Self {
        match value {
            Some("si") => Self::Si,
            _ => Self::Binary,
        }
    }

    fn base(self) -> f64 {
        match self {
            Self::Binary => 1024.0,
            Self::Si => 1000.0,
        }
    }
}

/// One size as the readout draws it: the unit step, and the value in hundredths of that unit (plain
/// bytes below the first step, which render as a whole number).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DisplayedSize {
    unit: u8,
    hundredths: u64,
}

/// The unit ladder's top rung (`PB`), past which values keep growing in that unit.
const LARGEST_UNIT: u8 = 5;

fn displayed_size(bytes: u64, format: FileSizeFormat) -> DisplayedSize {
    let base = format.base();
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= base && unit < LARGEST_UNIT {
        value /= base;
        unit += 1;
    }
    let hundredths = if unit == 0 {
        bytes
    } else {
        (value * 100.0).round() as u64
    };
    DisplayedSize { unit, hundredths }
}

/// Every figure a disk-space readout draws, at the resolution it draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DisplayedSpace {
    Bounded {
        free: DisplayedSize,
        total: DisplayedSize,
        /// The status bar's `(42%)`: whole percent free.
        free_percent: u64,
        /// The toast's `4.2%`: free percent in tenths.
        free_permille: u64,
        /// The usage bar's fill and colour band: whole percent used.
        used_percent: u64,
    },
    Unbounded {
        used: DisplayedSize,
    },
}

/// `space` as its readout draws it under `format`.
pub(super) fn displayed_space(space: &SpaceInfo, format: FileSizeFormat) -> DisplayedSpace {
    match *space {
        SpaceInfo::Bounded {
            total_bytes,
            available_bytes,
            ..
        } => {
            // `getUsedPercent` counts used as what isn't free, not the volume's own used figure.
            let (free_fraction, used_fraction) = if total_bytes == 0 {
                (0.0, 0.0)
            } else {
                let free = available_bytes as f64 / total_bytes as f64;
                let used = total_bytes.saturating_sub(available_bytes) as f64 / total_bytes as f64;
                (free, used)
            };
            DisplayedSpace::Bounded {
                free: displayed_size(available_bytes, format),
                total: displayed_size(total_bytes, format),
                free_percent: rounded_percent(free_fraction, 1.0),
                free_permille: rounded_percent(free_fraction, 10.0),
                used_percent: rounded_percent(used_fraction, 1.0),
            }
        }
        SpaceInfo::Unbounded { used_bytes } => DisplayedSpace::Unbounded {
            used: displayed_size(used_bytes, format),
        },
    }
}

/// `fraction` as a percent in steps of `1 / per_point`, clamped to 0–100 like the frontend's.
fn rounded_percent(fraction: f64, per_point: f64) -> u64 {
    (fraction.clamp(0.0, 1.0) * 100.0 * per_point).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;

    fn bounded(total: u64, available: u64) -> SpaceInfo {
        SpaceInfo::bounded(total, available)
    }

    fn same(a: &SpaceInfo, b: &SpaceInfo, format: FileSizeFormat) -> bool {
        displayed_space(a, format) == displayed_space(b, format)
    }

    #[test]
    fn a_change_below_the_readouts_last_digit_draws_the_same() {
        // 261.20 GB free: a GB readout moves in 0.01 GB (~10 MiB) steps, so a 2 MiB write is invisible.
        let before = bounded(926 * GIB, 261 * GIB + 205 * MIB);
        let after = bounded(926 * GIB, 261 * GIB + 203 * MIB);
        assert!(same(&before, &after, FileSizeFormat::Binary));
    }

    #[test]
    fn a_change_that_moves_the_last_digit_draws_differently() {
        let before = bounded(926 * GIB, 261 * GIB + 205 * MIB);
        let after = bounded(926 * GIB, 261 * GIB + 180 * MIB);
        assert!(!same(&before, &after, FileSizeFormat::Binary));
    }

    #[test]
    fn the_base_decides_where_the_digits_roll_over() {
        // 999.996 MB (SI) rounds to 1000.00 MB, while in binary the same bytes read 953.67 MB.
        let a = bounded(10 * GIB, 999_996_000);
        let b = bounded(10 * GIB, 999_994_000);
        assert!(!same(&a, &b, FileSizeFormat::Si));
        assert!(same(&a, &b, FileSizeFormat::Binary));
    }

    #[test]
    fn a_small_volume_shows_small_changes() {
        // 512.34 MB free: the readout moves in 0.01 MB (~10 KiB) steps.
        let before = bounded(GIB, 512 * MIB + 350 * 1024);
        let after = bounded(GIB, 512 * MIB + 300 * 1024);
        assert!(!same(&before, &after, FileSizeFormat::Binary));
    }

    #[test]
    fn the_toast_percent_counts_even_when_the_sizes_do_not() {
        // Free crosses 4.95% of a 1,000 GiB volume inside one 0.01 GiB step: the sizes read 49.50 GB
        // both times, while the toast's 4.9% turns 5.0%.
        let total = 1000 * GIB;
        let before = bounded(total, 49 * GIB + 511 * MIB);
        let after = bounded(total, 49 * GIB + 513 * MIB);
        assert!(!same(&before, &after, FileSizeFormat::Binary));
        match (
            displayed_space(&before, FileSizeFormat::Binary),
            displayed_space(&after, FileSizeFormat::Binary),
        ) {
            (
                DisplayedSpace::Bounded {
                    free: fa,
                    free_permille: a,
                    ..
                },
                DisplayedSpace::Bounded {
                    free: fb,
                    free_permille: b,
                    ..
                },
            ) => {
                assert_eq!(fa, fb, "the free size reads the same");
                assert_ne!(a, b, "the toast percent moves");
            }
            _ => unreachable!("both bounded"),
        }
    }

    #[test]
    fn an_unbounded_volume_draws_its_used_figure() {
        let a = SpaceInfo::Unbounded { used_bytes: 64 * MIB };
        let b = SpaceInfo::Unbounded {
            used_bytes: 64 * MIB + 1024,
        };
        let c = SpaceInfo::Unbounded { used_bytes: 70 * MIB };
        assert!(same(&a, &b, FileSizeFormat::Binary));
        assert!(!same(&a, &c, FileSizeFormat::Binary));
    }

    #[test]
    fn bytes_below_the_first_step_are_exact() {
        let a = SpaceInfo::Unbounded { used_bytes: 1000 };
        let b = SpaceInfo::Unbounded { used_bytes: 1001 };
        assert!(!same(&a, &b, FileSizeFormat::Binary));
    }

    #[test]
    fn an_unknown_total_reads_as_nothing_free() {
        // Guarded like the frontend, which never divides by a zero total.
        assert!(matches!(
            displayed_space(&bounded(0, 0), FileSizeFormat::Binary),
            DisplayedSpace::Bounded {
                free_percent: 0,
                used_percent: 0,
                ..
            }
        ));
    }

    #[test]
    fn the_setting_parses_with_binary_as_the_default() {
        assert_eq!(FileSizeFormat::from_setting(Some("si")), FileSizeFormat::Si);
        assert_eq!(FileSizeFormat::from_setting(Some("binary")), FileSizeFormat::Binary);
        assert_eq!(FileSizeFormat::from_setting(None), FileSizeFormat::Binary);
    }
}
