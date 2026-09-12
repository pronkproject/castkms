// SPDX-License-Identifier: GPL-2.0-only

//! Private packed host images, with no GEM handle or DMA-BUF export interface.

use crate::{
    gem,
    Driver, //
};
use kernel::{
    drm::{
        gem::shmem,
        Device, //
    },
    io::{
        io_project,
        Io,
        IoBase,
        SysMem, //
    },
    page::page_align,
    prelude::*, //
};

/// One exclusively owned image for the bounded host pool, not a capture destination.
///
/// The allocation and its page padding start cleared. Mutable access requires exclusive
/// ownership; neither its native object nor its mapping can escape through this interface.
pub(crate) struct Image {
    map: shmem::VMapOwned<gem::Object>,
    width: u32,
    height: u32,
    pitch: usize,
}

#[expect(dead_code)]
impl Image {
    pub(crate) fn new(device: &Device<Driver>, width: u32, height: u32) -> Result<Self> {
        if width == 0 || height == 0 || width > 1920 || height > 1080 {
            return Err(EINVAL);
        }
        let pitch = (width as usize).checked_mul(4).ok_or(EOVERFLOW)?;
        let bytes = pitch.checked_mul(height as usize).ok_or(EOVERFLOW)?;
        let size = page_align(bytes).ok_or(EOVERFLOW)?;
        if size > 8 * 1024 * 1024 {
            return Err(E2BIG);
        }
        let object = shmem::Object::<gem::Object>::new(device, size, Default::default(), ())?;
        let map = object.owned_vmap()?;
        // SAFETY: The new native allocation has no other pixel users or exported handles.
        // The owned mapping covers all `size` bytes, including the rounded page padding.
        unsafe { map.as_view().as_ptr().cast::<u8>().write_bytes(0, size) };
        Ok(Self {
            map,
            width,
            height,
            pitch,
        })
    }

    pub(crate) fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn row(&self, y: u32) -> Result<SysMem<'_, [u8]>> {
        if y >= self.height {
            return Err(EINVAL);
        }
        let start = y as usize * self.pitch;
        let end = start + self.pitch;
        let storage = self.map.as_view();
        // SAFETY: Construction checked and allocated every packed row. The mapping remains
        // owned for the returned borrow; this view excludes any final page padding.
        let bytes = unsafe {
            SysMem::new(core::ptr::slice_from_raw_parts_mut(
                storage.as_ptr().cast::<u8>(),
                self.pitch * self.height as usize,
            ))
        };
        Ok(io_project!(bytes, [try: start..end]))
    }

    pub(crate) fn write_row(&mut self, y: u32, pixels: &[u8]) -> Result {
        if pixels.len() != self.pitch {
            return Err(EINVAL);
        }
        self.row(y)?.copy_from_slice(pixels);
        Ok(())
    }

    pub(crate) fn read_row(&self, y: u32, pixels: &mut [u8]) -> Result {
        if pixels.len() != self.pitch {
            return Err(EINVAL);
        }
        self.row(y)?.copy_to_slice(pixels);
        Ok(())
    }
}
