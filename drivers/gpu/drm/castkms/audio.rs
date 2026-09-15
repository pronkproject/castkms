// SPDX-License-Identifier: GPL-2.0-only

//! Audio transport primitives, independent of display rendering and file descriptors.

#[cfg(any(CONFIG_DRM_CASTKMS_AUDIO, CONFIG_DRM_CASTKMS_KUNIT_TEST))]
mod buffer;
#[cfg(any(CONFIG_DRM_CASTKMS_AUDIO, CONFIG_DRM_CASTKMS_KUNIT_TEST))]
mod clock;
#[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
pub(crate) mod playback;
#[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
pub(crate) mod files;
#[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
pub(crate) mod provider;
#[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
mod source;
#[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
mod tap;

#[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
pub(crate) use source::{
    Attachment,
    Source, //
};

#[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
pub(crate) use files::create;

#[cfg(not(CONFIG_DRM_CASTKMS_AUDIO))]
pub(crate) fn create(
    _: &kernel::drm::Device<crate::Driver, kernel::drm::device::Registered>,
    _: &(),
    _: &mut kernel::uapi::drm_castkms_create_audio_capture,
    _: &kernel::drm::file::File<crate::File>,
) -> kernel::error::Result<u32> {
    Err(kernel::error::code::EOPNOTSUPP)
}

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
