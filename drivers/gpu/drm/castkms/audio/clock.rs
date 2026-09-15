// SPDX-License-Identifier: GPL-2.0-only

//! Integer frame clocks. Callers supply monotonic time in nanoseconds.

use super::{
    PERIOD_FRAMES,
    RATE, //
};

/// Convert without overflowing when monotonic uptime exceeds four days.
fn frames(ns: u64) -> u64 {
    ns / 1_000_000_000 * RATE + ns % 1_000_000_000 * RATE / 1_000_000_000
}

/// An independent capture clock continues producing silence while playback is idle.
pub(super) struct Capture {
    base_ns: u64,
    emitted: u64,
}

pub(super) struct Due {
    pub(super) frames: usize,
    pub(super) dropped: u64,
}

impl Capture {
    pub(super) fn new(now: u64) -> Self {
        Self {
            base_ns: now,
            emitted: 0,
        }
    }

    /// Account for full periods, bounding each callback to forty milliseconds of work.
    pub(super) fn advance(&mut self, now: u64) -> Due {
        let elapsed = frames(now.saturating_sub(self.base_ns));
        let due =
            elapsed.saturating_sub(self.emitted) / PERIOD_FRAMES as u64 * PERIOD_FRAMES as u64;
        self.emitted = self.emitted.saturating_add(due);
        let bounded = due.min(4 * PERIOD_FRAMES as u64);
        Due {
            frames: bounded as usize,
            dropped: due - bounded,
        }
    }
}
