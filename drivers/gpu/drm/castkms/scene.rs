// SPDX-License-Identifier: GPL-2.0-only

//! Owned display descriptions, without pixel-read permission or completion guarantees.

use super::Driver;
use core::num::NonZeroU64;
use kernel::{
    drm::{
        auth::MasterRef,
        kms::framebuffer::{
            dependencies::Dependencies,
            FramebufferRef, //
        }, //
    },
    prelude::*,
    sync::Arc, //
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
    pub(super) output: [u32; 2],
}

/// The framebuffer reference preserves storage lifetime, not the contents of that storage.
#[derive(Clone)]
pub(super) struct Scene {
    framebuffer: FramebufferRef<Driver>,
    geometry: Geometry,
    content: ContentSerial,
    // Historical attribution resolved by the accepted transaction, not live capture authority.
    owner: Option<MasterRef<Driver>>,
    producer: Option<Arc<Dependencies>>,
}

impl Scene {
    pub(super) fn framebuffer(&self) -> &kernel::drm::kms::framebuffer::Framebuffer<Driver> {
        &self.framebuffer
    }

    pub(super) fn geometry(&self) -> Geometry {
        self.geometry
    }

    pub(super) fn producer_result(&self) -> Result {
        let mut pending = false;
        if let Some(records) = &self.producer {
            for fence in records.iter() {
                match fence.status() {
                    kernel::dma_fence::Status::Pending => pending = true,
                    kernel::dma_fence::Status::Complete(result) => result?,
                }
            }
        }
        if pending {
            Err(EAGAIN)
        } else {
            Ok(())
        }
    }

    pub(super) fn content_serial(&self) -> ContentSerial {
        self.content
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) fn producer_failed(&self) -> bool {
        self.producer.as_ref().is_some_and(|records| {
            records
                .iter()
                .any(|fence| matches!(fence.status(), kernel::dma_fence::Status::Complete(Err(_))))
        })
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) fn producer_status(&self) -> Option<kernel::dma_fence::Status> {
        self.producer
            .as_ref()?
            .iter()
            .next()
            .map(|fence| fence.status())
    }

    pub(super) fn owner(&self) -> Option<&MasterRef<Driver>> {
        self.owner.as_ref()
    }

    pub(super) fn new(
        framebuffer: FramebufferRef<Driver>,
        geometry: Geometry,
        content: ContentSerial,
        owner: Option<MasterRef<Driver>>,
        producer: Option<Arc<Dependencies>>,
    ) -> Self {
        Self {
            framebuffer,
            geometry,
            content,
            owner,
            producer,
        }
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
