// SPDX-License-Identifier: GPL-2.0-only

//! Private packed host images, with no GEM handle or DMA-BUF export interface.

use super::{
    budget::{
        Budget,
        Charge, //
    },
    layout::Layout, //
};
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
    prelude::*,
    sync::Arc, //
};

/// One exclusively owned image for the bounded host pool, not a capture destination.
///
/// The allocation and its page padding start cleared. Mutable access requires exclusive
/// ownership; neither its native object nor its mapping can escape through this interface.
pub(crate) struct Image {
    map: shmem::VMapOwned<gem::Object>,
    // Release the mapping and its allocation before returning the reserved bytes.
    _charge: Charge,
    layout: Layout,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Image {
    pub(crate) fn new(
        device: &Device<Driver>,
        budget: &Arc<Budget>,
        layout: Layout,
    ) -> Result<Self> {
        let size = layout.size();
        let charge = budget.reserve(size)?;
        let object = shmem::Object::<gem::Object>::new(
            device, size, Default::default(), Default::default(),
        )?;
        let map = object.owned_vmap()?;
        // SAFETY: The new native allocation has no other pixel users or exported handles.
        // The owned mapping covers all `size` bytes, including the rounded page padding.
        unsafe { map.as_view().as_ptr().cast::<u8>().write_bytes(0, size) };
        Ok(Self {
            map,
            _charge: charge,
            layout,
        })
    }

    pub(crate) fn dimensions(&self) -> (u32, u32) {
        self.layout.dimensions()
    }

    pub(super) fn layout(&self) -> Layout {
        self.layout
    }

    fn pixels(&self) -> SysMem<'_, [u8]> {
        let storage = self.map.as_view();
        // SAFETY: The validated layout fits the owned allocation. The mapping remains
        // live for this borrow, which includes packed pixels but excludes page padding.
        unsafe {
            SysMem::new(core::ptr::slice_from_raw_parts_mut(
                storage.as_ptr().cast::<u8>(),
                self.layout.pixel_bytes(),
            ))
        }
    }

    fn row(&self, y: u32) -> Result<SysMem<'_, [u8]>> {
        let (_, height) = self.dimensions();
        let pitch = self.layout.pitch();
        if y >= height {
            return Err(EINVAL);
        }
        let start = y as usize * pitch;
        let end = start + pitch;
        Ok(io_project!(self.pixels(), [try: start..end]))
    }

    /// Copy packed pixels into independent host storage without copying page padding.
    pub(crate) fn copy_pixels(&self, output: &mut [u8]) -> Result {
        if output.len() != self.layout.pixel_bytes() {
            return Err(EINVAL);
        }
        self.pixels().copy_to_slice(output);
        Ok(())
    }

    pub(crate) fn write_row(&mut self, y: u32, pixels: &[u8]) -> Result {
        if pixels.len() != self.layout.pitch() {
            return Err(EINVAL);
        }
        self.row(y)?.copy_from_slice(pixels);
        Ok(())
    }

    /// Clear an exclusively owned allocation, including padding, before blank-image reuse.
    pub(super) fn clear(&mut self) {
        // SAFETY: The owned mapping covers the validated allocation. Exclusive access to
        // this private image excludes readers, and no GEM handle or DMA-BUF is exported.
        unsafe {
            self.map
                .as_view()
                .as_ptr()
                .cast::<u8>()
                .write_bytes(0, self.layout.size());
        }
    }

    pub(crate) fn read_row(&self, y: u32, pixels: &mut [u8]) -> Result {
        if pixels.len() != self.layout.pitch() {
            return Err(EINVAL);
        }
        self.row(y)?.copy_to_slice(pixels);
        Ok(())
    }

    /// Copy a matching source while the caller holds its synchronous CPU read claim.
    pub(super) fn copy_from(&mut self, source: &super::framebuffer::Mapping) -> Result {
        let (width, height) = self.dimensions();
        if source.width != width || source.height != height {
            return Err(EINVAL);
        }
        for y in 0..height as usize {
            let output = self.row(y as u32)?;
            if source.format == kernel::drm::fourcc::XRGB8888 {
                let mut bytes = [0; 1024];
                for start in (0..self.layout.pitch()).step_by(bytes.len()) {
                    let len = (self.layout.pitch() - start).min(bytes.len());
                    source.read(0, start, y, &mut bytes[..len])?;
                    io_project!(output, [try: start..start + len]).copy_from_slice(&bytes[..len]);
                }
                continue;
            }
            for x in 0..width as usize {
                let pixel = crate::formats::pixel(source.format, x, y, |plane, x, y, bytes| {
                    source.read(plane, x, y, bytes)
                })?;
                let offset = x * 4;
                io_project!(output, [try: offset..offset + 4])
                    .copy_from_slice(&pixel.to_le_bytes());
            }
        }
        Ok(())
    }
}
