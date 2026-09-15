// SPDX-License-Identifier: GPL-2.0

//! Configurable-rate playback position, independent of native runtime lifetime.

fn frames(ns: u64, rate: u32) -> u64 {
    let rate = rate as u64;
    (ns / 1_000_000_000)
        .saturating_mul(rate)
        .saturating_add(ns % 1_000_000_000 * rate / 1_000_000_000)
}

pub(super) struct Playback {
    rate: u32,
    base_ns: u64,
    base_frames: u64,
    running: bool,
}

impl Playback {
    pub(super) fn new(rate: u32) -> Self {
        Self {
            rate,
            base_ns: 0,
            base_frames: 0,
            running: false,
        }
    }

    pub(super) fn position(&self, now: u64) -> u64 {
        if self.running {
            self.base_frames
                .saturating_add(frames(now.saturating_sub(self.base_ns), self.rate))
        } else {
            self.base_frames
        }
    }

    pub(super) fn prepare(&mut self) {
        *self = Self::new(self.rate);
    }

    pub(super) fn start(&mut self, now: u64) {
        self.base_frames = 0;
        self.base_ns = now;
        self.running = true;
    }

    pub(super) fn resume(&mut self, now: u64) {
        if !self.running {
            self.base_ns = now;
            self.running = true;
        }
    }

    pub(super) fn stop(&mut self, now: u64) {
        self.base_frames = self.position(now);
        self.running = false;
    }
}

impl Playback {
    pub(super) fn running(&self) -> bool {
        self.running
    }
}

#[cfg(CONFIG_KUNIT)]
#[kernel::prelude::kunit_tests(rust_castkms_audio_playback_clock)]
mod tests {
    use super::*;

    #[test]
    fn pause_does_not_count_idle_time() {
        let mut clock = Playback::new(48_000);
        clock.start(1_000_000_000);
        clock.stop(1_010_000_000);
        assert_eq!(clock.position(9_000_000_000), 480);
        clock.resume(9_000_000_000);
        assert_eq!(clock.position(9_010_000_000), 960);
        clock.prepare();
        assert_eq!(clock.position(u64::MAX), 0);
    }

    #[test]
    fn long_uptime_does_not_overflow() {
        let mut clock = Playback::new(48_000);
        clock.start(0);
        assert_eq!(clock.position(10_000_000_000_000_000), 480_000_000_000);
        assert_eq!(clock.position(u64::MAX), 885_443_715_538_058);
    }

    #[test]
    fn alternate_rates_survive_prepare_and_saturate() {
        let mut clock = Playback::new(44_100);
        clock.start(0);
        assert_eq!(clock.position(1_000_000_000), 44_100);
        clock.prepare();
        clock.start(2_000_000_000);
        assert_eq!(clock.position(3_000_000_000), 44_100);
        assert_eq!(frames(u64::MAX, u32::MAX), u64::MAX);
    }
}
