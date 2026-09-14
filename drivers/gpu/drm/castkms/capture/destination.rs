// SPDX-License-Identifier: GPL-2.0-only

//! Checked host-linear destination storage, without capture permission or pixel access.

use crate::host_compositor::layout::Layout;
use kernel::{
    dma_buf::DmaBuf,
    drm::fourcc,
    prelude::*,
    sync::aref::ARef, //
};

const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;

/// One retained allocation and its checked single-plane XRGB8888 destination layout.
///
/// Construction does not map the buffer, wait for reuse, reserve a request or authorize
/// a write. The owner must exclude conflicting use and validate the recipient when it
/// admits delivery. Format metadata alone does not establish exporter CPU-map support.
pub(crate) struct Image {
    buffer: ARef<DmaBuf>,
    layout: Layout,
    pitch: usize,
    offset: usize,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Image {
    /// Validate the negotiated visible layout against all rows of supplied storage.
    ///
    /// Offsets and strides are bytes. The complete final row, including stride padding,
    /// must fit. Allocation bytes outside the described rows do not belong to this image.
    pub(crate) fn new(
        buffer: ARef<DmaBuf>,
        layout: Layout,
        format: u32,
        modifier: u64,
        pitch: usize,
        offset: usize,
    ) -> Result<Self> {
        if format != fourcc::XRGB8888 || modifier != fourcc::FORMAT_MOD_LINEAR {
            return Err(EOPNOTSUPP);
        }
        if pitch < layout.pitch() || pitch % 4 != 0 || offset % 4 != 0 {
            return Err(EINVAL);
        }
        let span = pitch
            .checked_mul(layout.dimensions().1 as usize)
            .ok_or(EOVERFLOW)?;
        let end = offset.checked_add(span).ok_or(EOVERFLOW)?;
        if end > buffer.size() {
            return Err(EINVAL);
        }
        // Bound per-delivery copying, including caller-supplied row padding.
        if span > MAX_IMAGE_BYTES {
            return Err(E2BIG);
        }
        Ok(Self {
            buffer,
            layout,
            pitch,
            offset,
        })
    }

    pub(crate) fn layout(&self) -> Layout {
        self.layout
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
