// SPDX-License-Identifier: GPL-2.0-only

//! Audio transport primitives, independent of display rendering and file descriptors.

#[cfg(any(CONFIG_DRM_CASTKMS_AUDIO, CONFIG_DRM_CASTKMS_KUNIT_TEST))]
mod buffer;

/// The playback and capture paths share one interleaved PCM format.
#[cfg(any(CONFIG_DRM_CASTKMS_AUDIO, CONFIG_DRM_CASTKMS_KUNIT_TEST))]
pub(crate) const FRAME_BYTES: usize = 4;
#[cfg(any(CONFIG_DRM_CASTKMS_AUDIO, CONFIG_DRM_CASTKMS_KUNIT_TEST))]
pub(crate) const BUFFER_FRAMES: usize = 65_536;

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
