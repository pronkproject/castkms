// SPDX-License-Identifier: GPL-2.0-only

//! Audio transport primitives, independent of display rendering and file descriptors.

#[cfg(any(CONFIG_DRM_CASTKMS_AUDIO, CONFIG_DRM_CASTKMS_KUNIT_TEST))]
mod buffer;
#[cfg(any(CONFIG_DRM_CASTKMS_AUDIO, CONFIG_DRM_CASTKMS_KUNIT_TEST))]
mod clock;
#[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
pub(crate) mod playback;

/// The playback and capture paths share one interleaved PCM format.
#[cfg(any(CONFIG_DRM_CASTKMS_AUDIO, CONFIG_DRM_CASTKMS_KUNIT_TEST))]
pub(crate) const RATE: u64 = 48_000;
#[cfg(any(CONFIG_DRM_CASTKMS_AUDIO, CONFIG_DRM_CASTKMS_KUNIT_TEST))]
pub(crate) const FRAME_BYTES: usize = 4;
#[cfg(any(CONFIG_DRM_CASTKMS_AUDIO, CONFIG_DRM_CASTKMS_KUNIT_TEST))]
pub(crate) const PERIOD_FRAMES: usize = 480;
#[cfg(any(CONFIG_DRM_CASTKMS_AUDIO, CONFIG_DRM_CASTKMS_KUNIT_TEST))]
pub(crate) const BUFFER_FRAMES: usize = 65_536;

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
