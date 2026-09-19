// SPDX-License-Identifier: GPL-2.0-only

//! Checked capture destination storage, without capture permission or pixel access.

use crate::capture::output_layout::Layout as OutputLayout;
#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
use crate::host_compositor::layout::Layout;
use kernel::{
    dma_buf::DmaBuf,
    prelude::*,
    sync::{aref::ARef, Arc}, //
};

/// One retained allocation and its checked single-plane destination layout.
///
/// Construction does not map the buffer, wait for reuse, reserve a request or authorize
/// a write. The owner must exclude conflicting use and validate the recipient when it
/// admits delivery. Format metadata alone does not establish exporter CPU-map support.
pub(crate) struct Image {
    buffer: ARef<DmaBuf>,
    dimensions: [u32; 2],
    pitch: usize,
    offset: usize,
    layout: OutputLayout,
    storage: Option<Storage>,
}

enum Storage {
    Host {
        _registration: crate::image_storage::Registration,
    },
    Delegated(Arc<crate::capture::provider::delegated_destination::Image>),
}

impl Image {
    /// Validate the negotiated visible layout against supplied storage.
    ///
    /// Offsets and strides are bytes. For linear layouts, the complete final row,
    /// including stride padding, must fit. Tiled layout bounds are checked by the
    /// native importer after this metadata and allocation identity are retained.
    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn new(
        buffer: ARef<DmaBuf>,
        layout: Layout,
        format: u32,
        modifier: u64,
        pitch: usize,
        offset: usize,
    ) -> Result<Self> {
        let (width, height) = layout.dimensions();
        Self::new_checked(buffer, [width, height], format, modifier, pitch, offset)
    }

    /// Validate recipient geometry without imposing the HOST compositor's axis limits.
    pub(crate) fn new_dimensions(
        buffer: ARef<DmaBuf>,
        dimensions: [u32; 2],
        format: u32,
        modifier: u64,
        pitch: usize,
        offset: usize,
    ) -> Result<Self> {
        if dimensions.contains(&0)
            || dimensions[0] > crate::execution::potential::MAX_DIMENSION
            || dimensions[1] > crate::execution::potential::MAX_DIMENSION
        {
            return Err(EINVAL);
        }
        Self::new_checked(buffer, dimensions, format, modifier, pitch, offset)
    }

    fn new_checked(
        buffer: ARef<DmaBuf>,
        dimensions: [u32; 2],
        format: u32,
        modifier: u64,
        pitch: usize,
        offset: usize,
    ) -> Result<Self> {
        let layout = OutputLayout::new(format, modifier)?;
        if pitch == 0 || pitch % 4 != 0 || offset % 4 != 0 {
            return Err(EINVAL);
        }
        if modifier == kernel::drm::fourcc::FORMAT_MOD_LINEAR {
            let row = (dimensions[0] as usize).checked_mul(4).ok_or(EOVERFLOW)?;
            if pitch < row {
                return Err(EINVAL);
            }
            let span = pitch.checked_mul(dimensions[1] as usize).ok_or(EOVERFLOW)?;
            let end = offset.checked_add(span).ok_or(EOVERFLOW)?;
            if end > buffer.size() {
                return Err(EINVAL);
            }
            // Bound per-delivery copying, including caller-supplied row padding.
            if span > crate::image_storage::MAX_BYTES {
                return Err(E2BIG);
            }
        } else if offset >= buffer.size() {
            return Err(EINVAL);
        }
        Ok(Self {
            buffer,
            dimensions,
            pitch,
            offset,
            layout,
            storage: None,
        })
    }

    pub(crate) fn dimensions(&self) -> [u32; 2] {
        self.dimensions
    }

    pub(crate) fn layout(&self) -> OutputLayout {
        self.layout
    }

    /// Retain the device-wide recipient role through detached destination access.
    pub(super) fn retain_storage(&mut self, storage: crate::image_storage::Registration) -> Result {
        if self.storage.is_some() {
            return Err(EALREADY);
        }
        if storage.dimensions() != self.dimensions
            || storage.buffers().len() != 1
            || !core::ptr::eq(&*storage.buffers()[0], &*self.buffer)
        {
            return Err(EINVAL);
        }
        self.storage = Some(Storage::Host {
            _registration: storage,
        });
        Ok(())
    }

    pub(super) fn retain_delegated(
        &mut self,
        scope: &crate::capture::provider::Delegated,
    ) -> Result {
        if self.storage.is_some() {
            return Err(EALREADY);
        }
        if self.dimensions != scope.dimensions() {
            return Err(EINVAL);
        }
        let image = scope.register_destination(
            &self.buffer,
            self.layout.format(),
            self.layout.modifier(),
            self.pitch,
            self.offset,
        )?;
        self.storage = Some(Storage::Delegated(image));
        Ok(())
    }

    pub(crate) fn delegated(
        &self,
    ) -> Result<Arc<crate::capture::provider::delegated_destination::Image>> {
        match &self.storage {
            Some(Storage::Delegated(image)) => Ok(image.clone()),
            _ => Err(EINVAL),
        }
    }

    pub(crate) fn buffer(&self) -> &DmaBuf {
        &self.buffer
    }

    pub(crate) fn pitch(&self) -> usize {
        self.pitch
    }

    pub(crate) fn offset(&self) -> usize {
        self.offset
    }
}
