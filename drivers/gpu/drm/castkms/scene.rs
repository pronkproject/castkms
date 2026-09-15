// SPDX-License-Identifier: GPL-2.0-only

//! Owned display descriptions, without pixel-read permission or completion guarantees.

mod configuration;
mod geometry;

pub(super) use geometry::Geometry;

pub(crate) use configuration::Configuration;

use super::Driver;
use core::num::NonZeroU64;
use kernel::{
    dma_resv::Reservation,
    drm::{
        auth::MasterRef,
        gem::BaseObject,
        kms::framebuffer::{
            dependencies::Dependencies,
            FramebufferRef, //
        }, //
    },
    prelude::*,
    sync::{aref::ARef, Arc}, //
};

/// A conservative content revision within one plane lifetime, not an ownership identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ContentSerial(NonZeroU64);

impl ContentSerial {
    pub(super) fn get(self) -> u64 {
        self.0.get()
    }

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

/// An active output, including a blank output with no framebuffer to retain.
#[derive(Clone)]
pub(super) struct Scene {
    primary: Option<Primary>,
    // Historical attribution resolved by the accepted transaction, not live capture authority.
    owner: Option<MasterRef<Driver>>,
}

/// The framebuffer reference preserves storage lifetime, not the contents of that storage.
#[derive(Clone)]
pub(super) struct Primary {
    framebuffer: FramebufferRef<Driver>,
    geometry: Geometry,
    content: ContentSerial,
    producer: Option<Arc<Dependencies>>,
}

impl Primary {
    pub(super) fn framebuffer(&self) -> &kernel::drm::kms::framebuffer::Framebuffer<Driver> {
        &self.framebuffer
    }

    pub(super) fn geometry(&self) -> Geometry {
        self.geometry
    }
}

impl Scene {
    /// Compare backing reservations without mapping pixels or acquiring a source read.
    pub(super) fn uses_reservation(&self, reservation: &Reservation) -> Result<bool> {
        if let Some(primary) = &self.primary {
            for plane in 0..primary.framebuffer.plane_count() {
                if core::ptr::eq(primary.framebuffer.object_at(plane)?.reservation(), reservation) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    pub(super) fn primary(&self) -> Option<&Primary> {
        self.primary.as_ref()
    }

    pub(super) fn producer_result(&self) -> Result {
        let mut pending = false;
        if let Some(records) = self
            .primary
            .as_ref()
            .and_then(|primary| primary.producer.as_ref())
        {
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

    /// Retain one native wait for the producer records acquired with this scene.
    pub(super) fn producer_completion(&self) -> Result<Option<ARef<kernel::dma_fence::Fence>>> {
        self.primary
            .as_ref()
            .and_then(|primary| primary.producer.as_ref())
            .map_or(Ok(None), |dependencies| dependencies.completion())
    }

    /// Blank output has no framebuffer content revision, not an unchanged revision.
    pub(super) fn content_serial(&self) -> Option<ContentSerial> {
        self.primary.as_ref().map(|primary| primary.content)
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) fn producer_failed(&self) -> bool {
        self.primary
            .as_ref()
            .and_then(|primary| primary.producer.as_ref())
            .is_some_and(|records| {
                records.iter().any(|fence| {
                    matches!(fence.status(), kernel::dma_fence::Status::Complete(Err(_)))
                })
            })
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) fn producer_status(&self) -> Option<kernel::dma_fence::Status> {
        self.primary
            .as_ref()?
            .producer
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
            primary: Some(Primary {
                framebuffer,
                geometry,
                content,
                producer,
            }),
            owner,
        }
    }

    pub(super) fn blank(owner: Option<MasterRef<Driver>>) -> Self {
        Self {
            primary: None,
            owner,
        }
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
