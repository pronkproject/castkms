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
    output::{CpuRead, Identity},
    scene::{
        Configuration,
        ContentSerial, //
    },
    Driver,
    Output, //
};
use kernel::{
    drm::auth::MasterRef,
    drm::preparation::Source,
    prelude::*,
    sync::aref::ARef,
    sync::Arc,
    time::{
        Instant,
        Monotonic, //
    }, //
};

/// A full private image whose compositor source is no longer claimed or retained.
///
/// Attribution describes the accepted scene; it does not authorize a capture recipient.
/// The pool slot remains occupied until this image is dropped, independently of KMS.
pub(crate) struct Completed {
    slot: Slot,
    output: Identity,
    configuration: Option<Configuration>,
    layout: Layout,
    content: Option<ContentSerial>,
    completed_at: Instant<Monotonic>,
    owner: Option<MasterRef<Driver>>,
}

pub(crate) enum Attempt {
    NoScene,
    Changed,
    AdmissionClosed(ARef<Source>),
    Image(Completed),
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

    pub(crate) fn content_serial(&self) -> Option<ContentSerial> {
        self.content
    }

    /// Time private CPU composition finished, not a KMS presentation timestamp.
    ///
    /// Reusing or copying the completed image does not make its pixels more recent.
    pub(crate) fn completed_at(&self) -> Instant<Monotonic> {
        self.completed_at
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
#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
pub(crate) fn current(output: &Output, pool: &Arc<Pool>) -> Result<Option<Completed>> {
    match current_checked(output, pool, || Ok(()))? {
        Attempt::NoScene => Ok(None),
        Attempt::Changed => Err(EAGAIN),
        Attempt::AdmissionClosed(_) => Err(EBUSY),
        Attempt::Image(image) => Ok(Some(image)),
    }
}

/// Retain caller admission across source claiming, after all mapping preparation.
///
/// The returned guard must exclude the caller's cutoff without waiting for device work.
/// It is released before source revalidation and copying; an admitted read completes
/// normally even if later admission closes. Destination storage remains source-independent.
pub(crate) fn current_checked<G>(
    output: &Output,
    pool: &Arc<Pool>,
    admit: impl FnOnce() -> Result<G>,
) -> Result<Attempt> {
    // An empty publication needs neither private storage nor source admission.
    if !output.has_scene() {
        return Ok(Attempt::NoScene);
    }
    let mut slot = pool.reserve()?;
    let layout = slot.with_image(|image| image.layout())?;
    let metadata = output.try_checked_cpu_scene(
        |scene| {
            if !scene.host_binding() {
                return Err(EOPNOTSUPP);
            }
            let mut layers = KVec::new();
            for (index, layer) in scene.layers().enumerate() {
                layers.push((index, layer), GFP_KERNEL)?;
            }
            // Break zpos ties in plane creation order, matching DRM object IDs.
            layers.sort_unstable_by_key(|(index, layer)| (layer.zpos, *index));
            let mut mappings = KVec::new();
            for (_, layer) in layers {
                let framebuffer = Framebuffer::new(layer.framebuffer(), layer.geometry())?;
                if framebuffer.dimensions() != layout.dimensions() {
                    return Err(EINVAL);
                }
                let mut mapping = framebuffer.prepare_mapping()?;
                mapping.color = layer.color.clone();
                mapping.yuv = layer.yuv;
                mappings.push(mapping, GFP_KERNEL)?;
            }
            Ok(mappings)
        },
        admit,
        |scene, configuration, mapping| -> Result<_> {
            let result = (|| {
                scene.producer_result()?;
                if !mapping.is_empty() {
                    slot.composite(mapping, scene.output_color.as_deref())?;
                } else {
                    let dimensions = configuration.as_ref().ok_or(EINVAL)?.dimensions();
                    if (dimensions[0], dimensions[1]) != layout.dimensions() {
                        return Err(EINVAL);
                    }
                    if scene.output_color.is_none() {
                        slot.clear()?;
                    } else {
                        slot.composite(mapping, scene.output_color.as_deref())?;
                    }
                }
                Ok((
                    scene.content_serial(),
                    scene.owner().cloned(),
                    configuration.clone(),
                ))
            })();
            // End exporter CPU access before releasing the read claim. Source reuse
            // must not race cache maintenance, even when copying failed. Unmapping
            // still happens after the claim, outside reservation and modeset locks.
            let mut finished = Ok(());
            for mapping in mapping {
                if let Err(error) = mapping.finish() {
                    finished = Err(error);
                }
            }
            finished?;
            result
        },
    )?;
    let metadata = match metadata {
        CpuRead::NoScene => return Ok(Attempt::NoScene),
        CpuRead::Changed => return Ok(Attempt::Changed),
        CpuRead::AdmissionClosed(source) => return Ok(Attempt::AdmissionClosed(source)),
        CpuRead::Read(metadata) => metadata,
    };
    let (content, owner, configuration) = metadata?;
    Ok(Attempt::Image(Completed {
        slot,
        output: output.identity().clone(),
        configuration,
        layout,
        content,
        completed_at: Instant::now(),
        owner,
    }))
}
