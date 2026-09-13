// SPDX-License-Identifier: GPL-2.0-only

//! Synchronous source-to-private-image composition, without capture authorization or delivery.

use super::{
    framebuffer::Framebuffer,
    layout::Layout,
    pool::{
        Pool,
        Slot, //
    }, //
};
use crate::{
    output::Identity,
    scene::{
        Configuration,
        ContentSerial, //
    },
    Driver,
    Output, //
};
use kernel::{
    drm::auth::MasterRef,
    prelude::*,
    sync::Arc, //
};

/// A full private image whose compositor source is no longer claimed or retained.
///
/// Attribution describes the accepted scene; it does not authorize a capture recipient.
/// The pool slot remains occupied until this image is dropped, independently of KMS.
#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
pub(crate) struct Completed {
    slot: Slot,
    output: Identity,
    configuration: Option<Configuration>,
    layout: Layout,
    content: ContentSerial,
    owner: Option<MasterRef<Driver>>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Completed {
    /// Origin of the private image, independent of current scene or capture permission.
    pub(crate) fn output_identity(&self) -> &Identity {
        &self.output
    }

    /// Display interval of the pixels, not the output's configuration at observation time.
    pub(crate) fn configuration(&self) -> Option<&Configuration> {
        self.configuration.as_ref()
    }

    pub(crate) fn layout(&self) -> Layout {
        self.layout
    }

    pub(crate) fn read_row(&self, y: u32, pixels: &mut [u8]) -> Result {
        self.slot.with_image(|image| image.read_row(y, pixels))?
    }

    /// Copy only packed pixels into independently owned, exact-size host storage.
    ///
    /// The caller must authorize the recipient before exposing the destination.
    /// Copying a private result neither claims the compositor source nor grants access.
    pub(crate) fn copy_pixels(&self, pixels: &mut [u8]) -> Result {
        self.slot.with_image(|image| image.copy_pixels(pixels))?
    }

    pub(crate) fn content_serial(&self) -> ContentSerial {
        self.content
    }

    pub(crate) fn owner(&self) -> Option<&MasterRef<Driver>> {
        self.owner.as_ref()
    }
}

/// Compose without waiting for private storage or downstream destination reuse.
///
/// Call only from worker context, outside modeset and reservation locks. Storage and
/// source mapping are prepared before claiming pixels. Failure returns the slot without
/// publishing its partial contents; success releases all source access before returning.
pub(crate) fn current(output: &Output, pool: &Arc<Pool>) -> Result<Option<Completed>> {
    // An empty publication needs neither private storage nor source admission.
    if !output.has_scene() {
        return Ok(None);
    }
    let mut slot = pool.reserve()?;
    let layout = slot.with_image(|image| image.layout())?;
    let metadata = output.with_prepared_cpu_scene(
        |scene| {
            let framebuffer = Framebuffer::new(scene.framebuffer(), scene.geometry())?;
            if framebuffer.dimensions() != layout.dimensions() {
                return Err(EINVAL);
            }
            framebuffer.prepare_mapping()
        },
        |scene, configuration, mapping| -> Result<_> {
            scene.producer_result()?;
            slot.copy_from(mapping)?;
            Ok((
                scene.content_serial(),
                scene.owner().cloned(),
                configuration.clone(),
            ))
        },
    )?;
    let Some(metadata) = metadata else {
        return Ok(None);
    };
    let (content, owner, configuration) = metadata?;
    Ok(Some(Completed {
        slot,
        output: output.identity().clone(),
        configuration,
        layout,
        content,
        owner,
    }))
}
