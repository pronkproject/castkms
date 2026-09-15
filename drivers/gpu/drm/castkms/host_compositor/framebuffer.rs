// SPDX-License-Identifier: GPL-2.0-only

//! Checked storage and sampling geometry, without pixel-read authority.

use crate::{
    execution::host,
    scene::Geometry,
    Driver, //
};
use kernel::{
    drm::gem::shmem,
    drm::kms::framebuffer::{
        Framebuffer as KmsFramebuffer,
        FramebufferRef, //
    },
    io::{io_project, Io, IoBase},
    prelude::*, //
};

/// A retained linear framebuffer satisfying the host compositor's storage limits.
///
/// Construction checks storage and sampling only. Scene color policy, capture authority and
/// source-read accounting remain separate. Mapping storage does not authorize pixel access.
pub(crate) struct Framebuffer {
    image: FramebufferRef<Driver>,
}

impl Framebuffer {
    pub(crate) fn new(image: &KmsFramebuffer<Driver>, geometry: Geometry) -> Result<Self> {
        host::check_framebuffer(image, geometry)?;
        Ok(Self {
            image: image.to_owned_ref(),
        })
    }

    pub(crate) fn dimensions(&self) -> (u32, u32) {
        (self.image.width(), self.image.height())
    }

    /// Prepare owned mappings without reading or claiming the source.
    pub(super) fn prepare_mapping(&self) -> Result<Mapping> {
        let mut planes = KVec::new();
        for index in 0..self.image.plane_count() {
            let object = self.image.object_at(index)?;
            let storage = Storage::Native(object.owned_vmap()?);
            planes.push(
                MappedPlane {
                    storage,
                    offset: self.image.offset(index)? as usize,
                    pitch: self.image.pitch(index)? as usize,
                    layout: crate::formats::plane(self.image.format(), index)?,
                },
                GFP_KERNEL,
            )?;
        }
        Ok(Mapping {
            planes,
            width: self.image.width(),
            height: self.image.height(),
            format: self.image.format(),
        })
    }
}

enum Storage {
    Native(shmem::VMapOwned<crate::gem::Object>),
}

struct MappedPlane {
    storage: Storage,
    offset: usize,
    pitch: usize,
    layout: crate::formats::Plane,
}

/// Prepared mappings retain storage but do not grant permission to read a scene.
pub(super) struct Mapping {
    planes: KVec<MappedPlane>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) format: u32,
}

impl Mapping {
    pub(super) fn read(&self, plane: usize, x: usize, y: usize, bytes: &mut [u8]) -> Result {
        let plane = self.planes.get(plane).ok_or(EINVAL)?;
        if y >= plane.layout.rows(self.height)
            || x.checked_add(bytes.len()).ok_or(EOVERFLOW)? > plane.layout.row_bytes(self.width)
        {
            return Err(EINVAL);
        }
        let start = y
            .checked_mul(plane.pitch)
            .and_then(|v| v.checked_add(plane.offset))
            .and_then(|v| v.checked_add(x))
            .ok_or(EOVERFLOW)?;
        match &plane.storage {
            Storage::Native(map) => {
                let end = start.checked_add(bytes.len()).ok_or(EOVERFLOW)?;
                let memory = map.as_view();
                // SAFETY: The owned mapping covers its allocation. The projection
                // checks the requested byte range before exposing I/O operations.
                let memory = unsafe {
                    kernel::io::SysMem::new(core::ptr::slice_from_raw_parts_mut(
                        memory.as_ptr().cast::<u8>(),
                        memory.size(),
                    ))
                };
                io_project!(memory, [try: start..end]).copy_to_slice(bytes);
                Ok(())
            }
        }
    }

    /// Complete source CPU access before its read claim is released.
    pub(super) fn finish(&self) -> Result {
        Ok(())
    }
}
