// SPDX-License-Identifier: GPL-2.0-only

//! Exact packed destination layout selected by a capture consumer.

use kernel::{drm::fourcc, prelude::*};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    format: u32,
    modifier: u64,
}

impl Layout {
    pub(crate) const HOST: Self = Self {
        format: fourcc::XRGB8888,
        modifier: fourcc::FORMAT_MOD_LINEAR,
    };

    pub(crate) fn new(format: u32, modifier: u64) -> Result<Self> {
        if !matches!(
            format,
            fourcc::XRGB8888 | fourcc::ARGB8888 | fourcc::XBGR8888 | fourcc::ABGR8888
        ) || modifier == fourcc::FORMAT_MOD_INVALID
        {
            return Err(EOPNOTSUPP);
        }
        Ok(Self { format, modifier })
    }

    pub(crate) fn format(self) -> u32 {
        self.format
    }

    pub(crate) fn modifier(self) -> u64 {
        self.modifier
    }
}
