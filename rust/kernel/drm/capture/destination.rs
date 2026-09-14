// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Borrowed output layouts whose native buffer pointers cannot outlive their Rust owners.

use crate::{
    bindings,
    dma_buf::DmaBuf,
    error::to_result,
    prelude::*, //
};
use core::marker::PhantomData;

/// One image plane, not a KMS plane or an independent capture authorization.
#[derive(Clone, Copy)]
pub struct DestinationPlane<'a> {
    buffer: &'a DmaBuf,
    stride: u32,
    offset: u64,
}

impl<'a> DestinationPlane<'a> {
    /// Describe borrowed storage; [`Destination::new`] checks the combined metadata.
    pub fn new(buffer: &'a DmaBuf, stride: u32, offset: u64) -> Self {
        Self {
            buffer,
            stride,
            offset,
        }
    }

    /// The retained allocation, without permission to map or write its pixels.
    pub fn buffer(&self) -> &'a DmaBuf {
        self.buffer
    }

    /// Byte stride, interpreted according to the image format and modifier.
    pub fn stride(&self) -> u32 {
        self.stride
    }

    /// Byte offset into this plane's allocation.
    pub fn offset(&self) -> u64 {
        self.offset
    }
}

/// Checked metadata borrowing up to four writable DMA-BUF image planes.
///
/// No reference is acquired, no buffer is mapped and no reuse fence is waited on.
/// Providers must still validate the complete format/modifier layout, row extents,
/// allocation limits and supported operations. Validation grants no capture authority.
///
/// # Invariants
///
/// The active plane pointers borrow live DMA-BUFs for `'a`. Every field is initialized
/// and metadata shape and export write access satisfy the native validation contract.
pub struct Destination<'a> {
    raw: bindings::drm_capture_destination,
    _buffers: PhantomData<&'a DmaBuf>,
}

// SAFETY: Metadata is immutable and its pointers borrow Sync DMA-BUFs for the full lifetime.
unsafe impl Send for Destination<'_> {}
// SAFETY: Shared methods expose only immutable metadata and shared DMA-BUF borrows.
unsafe impl Sync for Destination<'_> {}

impl<'a> Destination<'a> {
    /// Validate borrowed plane metadata without retaining the caller's plane array.
    ///
    /// All buffers must have been exported for writing. The returned description borrows
    /// those buffers, not descriptor numbers or the temporary list used to describe them.
    pub fn new(
        dimensions: [u32; 2],
        format: u32,
        modifier: u64,
        planes: &[DestinationPlane<'a>],
    ) -> Result<Self> {
        if planes.is_empty() || planes.len() > bindings::DRM_CAPTURE_DESTINATION_MAX_PLANES as usize
        {
            return Err(EINVAL);
        }
        let mut raw = bindings::drm_capture_destination {
            width: dimensions[0],
            height: dimensions[1],
            format,
            modifier,
            num_planes: planes.len() as u32,
            ..Default::default()
        };
        for (index, plane) in planes.iter().enumerate() {
            raw.planes[index] = bindings::drm_capture_destination_plane {
                buffer: plane.buffer.as_raw(),
                stride: plane.stride,
                offset: plane.offset,
            };
        }
        // SAFETY: Each active pointer borrows a live DMA-BUF for 'a and all metadata is
        // initialized. Native validation reads only metadata and retained file access mode.
        to_result(unsafe { bindings::drm_capture_destination_validate(&raw) })?;
        Ok(Self {
            raw,
            _buffers: PhantomData,
        })
    }

    /// Visible image dimensions in pixels.
    pub fn dimensions(&self) -> [u32; 2] {
        [self.raw.width, self.raw.height]
    }

    /// DRM fourcc; provider support is not established by metadata validation alone.
    pub fn format(&self) -> u32 {
        self.raw.format
    }

    /// Explicit DRM format modifier.
    pub fn modifier(&self) -> u64 {
        self.raw.modifier
    }

    /// Number of borrowed image planes.
    pub fn num_planes(&self) -> usize {
        self.raw.num_planes as usize
    }

    /// Borrow one described plane without transferring storage or pixel authority.
    pub fn plane(&self, index: usize) -> Option<DestinationPlane<'_>> {
        if index >= self.num_planes() {
            return None;
        }
        let plane = &self.raw.planes[index];
        Some(DestinationPlane {
            // SAFETY: The type invariant retains every active plane buffer for 'a.
            // The returned borrow is restricted further to this description's borrow.
            buffer: unsafe { DmaBuf::from_raw(plane.buffer) },
            stride: plane.stride,
            offset: plane.offset,
        })
    }
}
