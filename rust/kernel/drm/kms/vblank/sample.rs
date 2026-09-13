// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Coherent observations of DRM's vblank counter and its associated timestamp.

use super::VblankDriverCrtc;
use crate::{
    bindings,
    drm::kms::crtc::{
        AsRawCrtc,
        Crtc, //
    },
    time::{
        Instant,
        Monotonic, //
    }, //
};

/// A vblank counter and timestamp read together from one controller.
///
/// The counter includes DRM's accounting for intervals missed during modesets. It is not a
/// count of captured images or proof that a physical display presented particular pixels.
/// The value retains neither the controller nor an enabled vblank reference.
#[derive(Clone, Copy)]
pub struct VblankSample {
    sequence: u64,
    timestamp: Option<Instant<Monotonic>>,
}

impl VblankSample {
    /// The software-maintained vblank counter, including accounted missed intervals.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// The corresponding monotonic timestamp, when DRM has recorded one.
    ///
    /// An uninitialized or reset timestamp is unavailable, not the time of this read.
    pub fn timestamp(&self) -> Option<Instant<Monotonic>> {
        self.timestamp
    }
}

impl<T: VblankDriverCrtc> Crtc<T> {
    /// Read the vblank counter and its corresponding timestamp as a coherent pair.
    ///
    /// Reading does not enable the clock, wait for a new interval, or acquire pixel access.
    /// A disabled controller remains readable while its DRM device is retained. Observations
    /// from different controllers do not share a counter namespace.
    ///
    /// Writes preceding [`Self::handle_vblank`] are visible after observing the counter value
    /// from that call or a later value, as guaranteed by the native DRM helper.
    pub fn vblank_count_and_time(&self) -> VblankSample {
        let mut time = 0;
        // SAFETY: This initialized CRTC and its vblank storage remain alive for the borrow.
        // The native helper synchronizes the pair; `time` is valid writable stack storage.
        let sequence =
            unsafe { bindings::drm_crtc_vblank_count_and_time(self.as_raw(), &mut time) };
        let timestamp = if time > 0 {
            // SAFETY: A positive ktime_t is within Instant's nonnegative i64 range. DRM
            // records its vblank timestamps against the monotonic clock.
            Some(unsafe { Instant::from_ktime(time) })
        } else {
            None
        };
        VblankSample {
            sequence,
            timestamp,
        }
    }
}
