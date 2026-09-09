// SPDX-License-Identifier: GPL-2.0-only

//! Requested framebuffer selection, independent of content changes and authority.

use kernel::drm::kms::{
    atomic::PlaneInput,
    framebuffer::Framebuffer, //
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Selection {
    RetainedFramebuffer,
    DifferentFramebuffer,
}

impl Selection {
    pub(crate) fn for_update(
        input: PlaneInput<'_, crate::Driver>,
        previous: Option<&Framebuffer<crate::Driver>>,
        final_image: Option<&Framebuffer<crate::Driver>>,
    ) -> Self {
        let requested = match input {
            PlaneInput::Included {
                framebuffer,
                framebuffer_assigned: true,
            } => framebuffer,
            _ => None,
        };
        Self::classify(
            requested.map(core::ptr::from_ref),
            previous.map(core::ptr::from_ref),
            final_image.map(core::ptr::from_ref),
        )
    }

    fn classify<I: Eq>(requested: Option<I>, previous: Option<I>, final_image: Option<I>) -> Self {
        // Selection requires the requested non-null image to survive validation unchanged.
        if requested.is_some() && requested == final_image && final_image != previous {
            Self::DifferentFramebuffer
        } else {
            Self::RetainedFramebuffer
        }
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
