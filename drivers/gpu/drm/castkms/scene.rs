// SPDX-License-Identifier: GPL-2.0-only

//! Owned display descriptions, without pixel-read permission or completion guarantees.

use super::Driver;
use core::num::NonZeroU64;
use kernel::{
    drm::{
        auth::MasterRef,
        kms::framebuffer::FramebufferRef, //
    },
    prelude::*, //
};

/// A conservative content revision within one plane lifetime, not an ownership identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ContentSerial(NonZeroU64);

impl ContentSerial {
    /// Derive a candidate from the last accepted state without consuming a sequence number.
    pub(super) fn for_update(previous: Option<Self>, has_scene: bool) -> Result<Option<Self>> {
        // Blanking requires no content identity and must remain possible at exhaustion.
        if !has_scene {
            return Ok(previous);
        }
        let value = previous.map_or(0, |serial| serial.0.get());
        let next = value.checked_add(1).ok_or(EOVERFLOW)?;
        Ok(Some(Self(NonZeroU64::new(next).ok_or(EOVERFLOW)?)))
    }
}

/// Requested source coordinates are retained in DRM's unsigned 16.16 representation.
#[derive(Clone, Copy)]
pub(super) struct Geometry {
    pub(super) source: [u32; 4],
    pub(super) destination: [u32; 2],
}

/// The framebuffer reference preserves storage lifetime, not the contents of that storage.
pub(super) struct Scene {
    _framebuffer: FramebufferRef<Driver>,
    _source: [u32; 4],
    _destination: [u32; 2],
    _content: ContentSerial,
    // Historical attribution resolved by the accepted transaction, not live capture authority.
    _owner: Option<MasterRef<Driver>>,
}

impl Scene {
    pub(super) fn new(
        framebuffer: FramebufferRef<Driver>,
        geometry: Geometry,
        content: ContentSerial,
        owner: Option<MasterRef<Driver>>,
    ) -> Self {
        Self {
            _framebuffer: framebuffer,
            _source: geometry.source,
            _destination: geometry.destination,
            _content: content,
            _owner: owner,
        }
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
