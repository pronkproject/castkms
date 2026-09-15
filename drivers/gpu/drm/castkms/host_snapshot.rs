// SPDX-License-Identifier: GPL-2.0-only

//! Independent immutable copies of completed host images, without ambient export authority.

use crate::{
    gem,
    host_compositor::{
        compose::Completed,
        layout::Layout, //
    },
    output::Identity,
    scene::{
        Configuration,
        ContentSerial, //
    },
    Driver, //
};
use kernel::{
    drm::{
        auth::MasterRef,
        gem::{
            shmem,
            BaseObject,
            ExportAccess,
            ObjectRef, //
        },
        Device, //
    },
    fs::File,
    io::{
        Io,
        IoBase,
        SysMem, //
    },
    prelude::*,
    sync::{
        aref::ARef,
        Arc, //
    },
    time::{
        Instant,
        Monotonic, //
    }, //
};

/// One output's startup copies, including allocations retained after a failed candidate.
///
/// Keep the same budget across candidates. It is separate from the reusable host pool;
/// exhausting it omits the optional snapshot rather than waiting for a previous recipient.
pub(crate) struct Budget(Arc<gem::budget::Budget>);

impl Budget {
    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn reserve_for_test(&self, device: &Device<Driver>, bytes: usize) -> Result<impl Sized> {
        // An unmapped shmem object reserves credit without populating pixel pages.
        gem::Object::new_budgeted(device, &self.0, bytes)
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn limited_for_test(bytes: usize) -> Result<Self> {
        Ok(Self(gem::budget::Budget::new(bytes)?))
    }

    pub(crate) fn new() -> Result<Self> {
        Ok(Self(gem::budget::Budget::new(512 * 1024 * 1024)?))
    }
}

/// A fresh copy retaining its actual origin, not a reusable host slot or compositor source.
///
/// No mutable storage or GEM handle is exposed. Creating this private copy does not authorize
/// a recipient; an unpublished DMA-BUF file may be created only so the startup controller can
/// install it after checking current authority, configuration and candidate identity. Retained
/// metadata remains historical if the display changes while the copy is made.
pub(crate) struct Snapshot {
    object: ObjectRef<shmem::Object<gem::Object>>,
    layout: Layout,
    output: Identity,
    configuration: Option<Configuration>,
    content: Option<ContentSerial>,
    completed_at: Instant<Monotonic>,
    owner: Option<MasterRef<Driver>>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Snapshot {
    /// Copy outside modeset, reservation, and permission locks, after source access ended.
    pub(crate) fn new(device: &Device<Driver>, budget: &Budget, image: &Completed) -> Result<Self> {
        let layout = image.layout();
        let object = gem::Object::new_budgeted(device, &budget.0, layout.size())?;
        {
            let map = object.vmap::<0>()?;
            // SAFETY: This fresh allocation has no exported handles or other pixel users.
            // The mapping covers the complete checked allocation, including page padding.
            let bytes = unsafe {
                core::slice::from_raw_parts_mut(map.as_view().as_ptr().cast::<u8>(), layout.size())
            };
            bytes.fill(0);
            image.copy_pixels(&mut bytes[..layout.pixel_bytes()])?;
        }
        Ok(Self {
            object,
            layout,
            output: image.output_identity().clone(),
            configuration: image.configuration().cloned(),
            content: image.content_serial(),
            completed_at: image.completed_at(),
            owner: image.owner().cloned(),
        })
    }

    pub(crate) fn layout(&self) -> Layout {
        self.layout
    }

    pub(crate) fn output_identity(&self) -> &Identity {
        &self.output
    }

    pub(crate) fn configuration(&self) -> Option<&Configuration> {
        self.configuration.as_ref()
    }

    pub(crate) fn content_serial(&self) -> Option<ContentSerial> {
        self.content
    }

    /// Completion time of the original image, not when this independent copy was made.
    pub(crate) fn completed_at(&self) -> Instant<Monotonic> {
        self.completed_at
    }

    pub(crate) fn owner(&self) -> Option<&MasterRef<Driver>> {
        self.owner.as_ref()
    }

    pub(crate) fn content_serial_value(&self) -> u64 {
        self.content.map_or(0, ContentSerial::get)
    }

    /// Create an unpublished read-only DMA-BUF file for this immutable copy.
    ///
    /// The returned file owns its export reference but grants no userspace access until a
    /// caller installs it. The caller must revalidate recipient authority at installation.
    pub(crate) fn export_file(&self) -> Result<ARef<File>> {
        Ok(self
            .object
            .export_dma_buf(ExportAccess::ReadOnly)?
            .to_file())
    }

    /// Read the immutable private copy, without conferring permission to deliver its pixels.
    pub(crate) fn copy_pixels(&self, pixels: &mut [u8]) -> Result {
        if pixels.len() != self.layout.pixel_bytes() {
            return Err(EINVAL);
        }
        self.copy_bytes(pixels)
    }

    fn copy_bytes(&self, bytes: &mut [u8]) -> Result {
        let map = self.object.vmap::<0>()?;
        // SAFETY: Both callers bound the slice to the private allocation. No writer or
        // raw storage reference escapes this type; the mapping remains live for the copy.
        let storage = unsafe {
            SysMem::new(core::ptr::slice_from_raw_parts_mut(
                map.as_view().as_ptr().cast::<u8>(),
                bytes.len(),
            ))
        };
        storage.copy_to_slice(bytes);
        Ok(())
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn copy_allocation_for_test(&self, bytes: &mut [u8]) -> Result {
        if bytes.len() != self.layout.size() {
            return Err(EINVAL);
        }
        self.copy_bytes(bytes)
    }
}
