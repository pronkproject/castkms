// SPDX-License-Identifier: GPL-2.0

//! Driver-supplied PCM registration parameters.

use crate::{bindings, prelude::*, str::CStr};

/// Integer PCM storage formats supported by the playback registration.
#[derive(Clone, Copy)]
pub enum Format {
    /// Signed 16-bit little endian.
    S16Le,
    /// Signed 24-bit little endian in a 32-bit container.
    S24Le,
    /// Signed 32-bit little endian.
    S32Le,
}

impl Format {
    pub(super) fn native(self) -> u32 {
        match self {
            Self::S16Le => bindings::SNDRV_PCM_FORMAT_S16_LE,
            Self::S24Le => bindings::SNDRV_PCM_FORMAT_S24_LE,
            Self::S32Le => bindings::SNDRV_PCM_FORMAT_S32_LE,
        }
    }

    pub(super) fn bytes(self) -> usize {
        match self {
            Self::S16Le => 2,
            Self::S24Le | Self::S32Le => 4,
        }
    }
}

/// Driver-selected playback geometry and advertised capabilities.
#[derive(Clone, Copy)]
pub struct Config {
    /// One advertised PCM sample format.
    pub format: Format,
    /// One advertised sample rate, in frames per second.
    pub rate: u32,
    /// One advertised channel count, from one through eight.
    pub channels: u32,
    /// Largest ALSA playback allocation, in bytes.
    pub buffer_bytes_max: u32,
    /// Smallest period, in bytes.
    pub period_bytes_min: u32,
    /// Largest period, in bytes.
    pub period_bytes_max: u32,
    /// Smallest number of periods per buffer.
    pub periods_min: u32,
    /// Largest number of periods per buffer.
    pub periods_max: u32,
    /// Advertise pause support supplied by the driver.
    pub pause: bool,
}

impl Config {
    pub(super) fn frame_bytes(self) -> usize {
        self.format.bytes() * self.channels as usize
    }

    pub(super) fn validate(self) -> Result {
        if self.rate == 0
            || !(1..=8).contains(&self.channels)
            || self.periods_min < 2
            || self.periods_max < self.periods_min
            || self.period_bytes_min < self.frame_bytes() as u32
            || self.period_bytes_max < self.period_bytes_min
            || self.buffer_bytes_max / self.periods_min < self.period_bytes_min
            || self.period_bytes_max > self.buffer_bytes_max
        {
            return Err(EINVAL);
        }
        Ok(())
    }

    pub(super) fn hardware(self) -> bindings::snd_pcm_hardware {
        bindings::snd_pcm_hardware {
            info: bindings::SNDRV_PCM_INFO_INTERLEAVED
                | bindings::SNDRV_PCM_INFO_MMAP
                | bindings::SNDRV_PCM_INFO_MMAP_VALID
                | if self.pause {
                    bindings::SNDRV_PCM_INFO_PAUSE
                } else {
                    0
                },
            formats: 1 << self.format.native(),
            rates: bindings::SNDRV_PCM_RATE_KNOT,
            rate_min: self.rate,
            rate_max: self.rate,
            channels_min: self.channels,
            channels_max: self.channels,
            buffer_bytes_max: self.buffer_bytes_max as _,
            period_bytes_min: self.period_bytes_min as _,
            period_bytes_max: self.period_bytes_max as _,
            periods_min: self.periods_min,
            periods_max: self.periods_max,
            ..pin_init::zeroed()
        }
    }
}

/// Names copied into the native ALSA card at registration.
pub struct Identity<'a> {
    /// Stable card identifier.
    pub id: &'a CStr,
    /// Driver identifier.
    pub driver: &'a CStr,
    /// Human-readable card and PCM name.
    pub name: &'a CStr,
}

#[cfg(CONFIG_KUNIT)]
#[crate::prelude::kunit_tests(rust_snd_pcm_configuration)]
mod tests {
    use super::*;

    pub(super) fn config() -> Config {
        Config {
            format: Format::S32Le,
            rate: 44_100,
            channels: 6,
            buffer_bytes_max: 96_000,
            period_bytes_min: 240,
            period_bytes_max: 24_000,
            periods_min: 2,
            periods_max: 16,
            pause: false,
        }
    }

    #[test]
    fn geometry_and_capabilities_are_driver_selected() -> Result {
        let config = config();
        config.validate()?;
        assert_eq!(config.frame_bytes(), 24);
        let hw = config.hardware();
        assert_eq!(hw.rate_min, 44_100);
        assert_eq!(hw.channels_max, 6);
        assert_eq!(hw.formats, 1 << bindings::SNDRV_PCM_FORMAT_S32_LE);
        assert_eq!(Format::S24Le.bytes(), 4);
        assert_eq!(hw.info & bindings::SNDRV_PCM_INFO_PAUSE, 0);
        assert_eq!(
            hw.info & bindings::SNDRV_PCM_INFO_HAS_LINK_ESTIMATED_ATIME,
            0
        );
        Ok(())
    }

    #[test]
    fn invalid_geometry_is_rejected_before_registration() {
        assert!(Config {
            rate: 0,
            ..config()
        }
        .validate()
        .is_err());
        assert!(Config {
            channels: 0,
            ..config()
        }
        .validate()
        .is_err());
        assert!(Config {
            channels: u32::MAX,
            ..config()
        }
        .validate()
        .is_err());
        assert!(Config {
            periods_min: 0,
            ..config()
        }
        .validate()
        .is_err());
    }
}
