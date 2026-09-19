// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Offered final-image metadata, without storage ownership or pixel permission.

use crate::{
    drm::fourcc,
    error::to_result,
    fs::File,
    prelude::*, //
};
use core::num::{
    NonZeroU32,
    NonZeroU64, //
};

/// One client's named capture configuration, not a buffer or authorization token.
///
/// The provider must retain the named configuration, avoid reusing names and check
/// permission when opening it. Dimensions and format describe visible pixels, not
/// destination allocation, strides, mapping support or reuse synchronization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Description {
    id: NonZeroU64,
    dimensions: [NonZeroU32; 2],
    refresh_millihz: NonZeroU32,
    mode_flags: u32,
    format: NonZeroU32,
    modifier: u64,
    max_requests: NonZeroU32,
}

/// Exact storage accepted by a capture consumer, or the provider default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RequestedLayout {
    format: u32,
    modifier: u64,
}

impl RequestedLayout {
    /// Request one exact packed image format and modifier.
    pub fn exact(format: u32, modifier: u64) -> Result<Self> {
        if format == 0 || modifier == fourcc::FORMAT_MOD_INVALID {
            return Err(EINVAL);
        }
        Ok(Self { format, modifier })
    }

    /// Requested DRM fourcc, or zero for the provider default.
    pub fn format(self) -> u32 {
        self.format
    }
    /// Requested DRM format modifier.
    pub fn modifier(self) -> u64 {
        self.modifier
    }

    pub(super) fn from_raw(raw: &bindings::drm_capture_layout) -> Self {
        Self {
            format: raw.format,
            modifier: raw.modifier,
        }
    }

    fn raw(self) -> bindings::drm_capture_layout {
        bindings::drm_capture_layout {
            format: self.format,
            modifier: self.modifier,
        }
    }
}

impl Description {
    /// Validate metadata without reserving storage or asserting provider support.
    pub fn new(
        id: u64,
        dimensions: [u32; 2],
        refresh_millihz: u32,
        mode_flags: u32,
        format: u32,
        modifier: u64,
        max_requests: u32,
    ) -> Result<Self> {
        if modifier == fourcc::FORMAT_MOD_INVALID {
            return Err(EINVAL);
        }
        Ok(Self {
            id: NonZeroU64::new(id).ok_or(EINVAL)?,
            dimensions: [
                NonZeroU32::new(dimensions[0]).ok_or(EINVAL)?,
                NonZeroU32::new(dimensions[1]).ok_or(EINVAL)?,
            ],
            refresh_millihz: NonZeroU32::new(refresh_millihz).ok_or(EINVAL)?,
            mode_flags,
            format: NonZeroU32::new(format).ok_or(EINVAL)?,
            modifier,
            max_requests: NonZeroU32::new(max_requests).ok_or(EINVAL)?,
        })
    }

    /// Query a capture-client file through its provider, without userspace memory access.
    ///
    /// Native dispatch checks the file role and serializes its provider callback.
    /// Other files return EINVAL; absent support returns EOPNOTSUPP. Call outside
    /// DRM and authority locks. Success does not authorize a subsequent operation.
    pub fn query(client: &File) -> Result<Self> {
        Self::query_layout(client, RequestedLayout::default())
    }

    /// Query one exact layout. Unsupported tuples fail without changing a stream.
    pub fn query_layout(client: &File, layout: RequestedLayout) -> Result<Self> {
        let mut result = bindings::drm_capture_description::default();
        let requested = layout.raw();
        // SAFETY: The borrowed file remains live, and output storage is writable for the
        // entire call. Native dispatch validates the file before accessing provider data.
        to_result(unsafe {
            bindings::drm_capture_client_describe(client.as_ptr(), &requested, &mut result)
        })?;
        Self::new(
            result.id,
            [result.width, result.height],
            result.refresh_millihz,
            result.mode_flags,
            result.format,
            result.modifier,
            result.max_requests,
        )
    }

    /// Client-local offer name, with no meaning on another client.
    pub fn id(self) -> u64 {
        self.id.get()
    }

    /// Visible width and height in pixels.
    pub fn dimensions(self) -> [u32; 2] {
        self.dimensions.map(NonZeroU32::get)
    }

    /// Accepted display refresh rate in millihertz.
    pub fn refresh_millihz(self) -> u32 {
        self.refresh_millihz.get()
    }

    /// Accepted DRM mode flags.
    pub fn mode_flags(self) -> u32 {
        self.mode_flags
    }

    /// DRM fourcc of the offered image.
    pub fn format(self) -> u32 {
        self.format.get()
    }

    /// DRM format modifier of the offered image.
    pub fn modifier(self) -> u64 {
        self.modifier
    }

    /// Maximum requests per stream, without reserving any available credit.
    pub fn max_requests(self) -> u32 {
        self.max_requests.get()
    }

    pub(super) fn raw(self) -> bindings::drm_capture_description {
        let [width, height] = self.dimensions();
        bindings::drm_capture_description {
            id: self.id(),
            width,
            height,
            refresh_millihz: self.refresh_millihz(),
            mode_flags: self.mode_flags(),
            format: self.format(),
            max_requests: self.max_requests(),
            modifier: self.modifier(),
        }
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
