// SPDX-License-Identifier: GPL-2.0-only

//! Synchronous source-to-private-image composition, without capture authorization or delivery.

use super::{
    framebuffer::Framebuffer,
    pool::{
        Pool,
        Slot, //
    }, //
};
use crate::{
    output::Output,
    scene::{
        ContentSerial,
        Scene, //
    },
    Driver, //
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
    content: ContentSerial,
    owner: Option<MasterRef<Driver>>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Completed {
    pub(crate) fn read_row(&self, y: u32, pixels: &mut [u8]) -> Result {
        self.slot.with_image(|image| image.read_row(y, pixels))?
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
pub(crate) fn current(output: &Output<Scene>, pool: &Arc<Pool>) -> Result<Option<Completed>> {
    let mut slot = pool.reserve()?;
    let dimensions = slot.with_image(|image| image.dimensions())?;
    let metadata = output.with_prepared_cpu_scene(
        |scene| {
            let framebuffer = Framebuffer::new(scene.framebuffer(), scene.geometry())?;
            if framebuffer.dimensions() != dimensions {
                return Err(EINVAL);
            }
            framebuffer.prepare_mapping()
        },
        |scene, mapping| -> Result<_> {
            scene.producer_result()?;
            slot.copy_from(mapping)?;
            Ok((scene.content_serial(), scene.owner().cloned()))
        },
    )?;
    let Some(metadata) = metadata else {
        return Ok(None);
    };
    let (content, owner) = metadata?;
    Ok(Some(Completed {
        slot,
        content,
        owner,
    }))
}
