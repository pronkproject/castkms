// SPDX-License-Identifier: GPL-2.0-only

//! Nonblocking reservation of the two private host images for an output.

use super::image::Image;
use crate::Driver;
use kernel::{
    drm::Device,
    prelude::*,
    sync::{
        Arc,
        Mutex, //
    }, //
};

/// Two preallocated images, with no allocation or wait for image reuse during reservation.
///
/// Each image is at most 8 MiB, so the pool is at most 16 MiB. A worker must reserve a
/// slot before claiming a compositor source. Shutdown discards free images immediately;
/// outstanding slots remain private and release their storage when returned.
#[pin_data]
pub(crate) struct Pool {
    #[pin]
    images: Mutex<Option<[Option<Image>; 2]>>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Pool {
    pub(crate) fn new(device: &Device<Driver>, width: u32, height: u32) -> Result<Arc<Self>> {
        let first = Image::new(device, width, height)?;
        let second = Image::new(device, width, height)?;
        Arc::pin_init(
            pin_init!(Self {
                images <- kernel::new_mutex!(Some([Some(first), Some(second)])),
            }),
            GFP_KERNEL,
        )
    }

    pub(crate) fn reserve(self: &Arc<Self>) -> Result<Slot> {
        let mut state = self.images.lock();
        let images = (*state).as_mut().ok_or(ENODEV)?;
        for (index, entry) in images.iter_mut().enumerate() {
            if let Some(image) = entry.take() {
                return Ok(Slot {
                    pool: self.clone(),
                    index,
                    image: Some(image),
                });
            }
        }
        Err(EBUSY)
    }

    pub(crate) fn close(&self) {
        let images = self.images.lock().take();
        // Unmapping and object destruction may enter DRM.
        drop(images);
    }
}

/// Exclusive ownership of one pool image until the slot is dropped.
///
/// The slot is not cloneable. Its private index comes from the pool's two-entry array,
/// and its image is taken only by Drop. Access does not confer capture authorization.
pub(crate) struct Slot {
    pool: Arc<Pool>,
    index: usize,
    image: Option<Image>,
}

impl Slot {
    pub(crate) fn with_image<R>(&self, access: impl FnOnce(&Image) -> R) -> Result<R> {
        Ok(access(self.image.as_ref().ok_or(EIO)?))
    }

    pub(super) fn copy_from(
        &mut self,
        source: &kernel::drm::kms::framebuffer::FramebufferVMapOwned<crate::gem::Object>,
    ) -> Result {
        self.image.as_mut().ok_or(EIO)?.copy_from(source)
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn write_row(&mut self, y: u32, pixels: &[u8]) -> Result {
        self.image.as_mut().ok_or(EIO)?.write_row(y, pixels)
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let Some(image) = self.image.take() else {
            return;
        };
        let discarded = {
            let mut state = self.pool.images.lock();
            match (*state).as_mut() {
                Some(images) => images[self.index].replace(image),
                None => Some(image),
            }
        };
        drop(discarded);
    }
}
