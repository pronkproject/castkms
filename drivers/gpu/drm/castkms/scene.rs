// SPDX-License-Identifier: GPL-2.0-only

//! Owned display descriptions, without pixel-read permission or completion guarantees.

mod configuration;
mod geometry;

pub(super) use geometry::Geometry;

pub(crate) use configuration::Configuration;

use super::Driver;
use crate::execution::constraints::backend::Binding;
use core::num::NonZeroU64;
use kernel::{
    dma_resv::Reservation,
    drm::{
        auth::MasterRef,
        constraints::OpaqueEntry,
        gem::BaseObject,
        kms::framebuffer::{
            dependencies::Dependencies,
            FramebufferRef, //
        }, //
    },
    prelude::*,
    sync::{aref::ARef, Arc}, //
};

/// A conservative content revision within one output lifetime, not an ownership identity.
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
    pub(super) output_color: Option<Arc<crate::color::OutputColor>>,
    layers: [Option<Arc<Primary>>; MAX_PLANES],
    content: Option<ContentSerial>,
    // Historical attribution resolved by the accepted transaction, not live capture authority.
    owner: Option<MasterRef<Driver>>,
    constraints: Option<ARef<OpaqueEntry>>,
    binding: Option<crate::execution::constraints::backend::Binding>,
}

/// The framebuffer reference preserves storage lifetime, not the contents of that storage.
#[derive(Clone)]
pub(super) struct Primary {
    pub(super) yuv: (
        kernel::drm::kms::plane::ColorEncoding,
        kernel::drm::kms::plane::ColorRange,
    ),
    pub(super) color: Option<Arc<crate::color::Pipeline>>,
    pub(super) framebuffer: FramebufferRef<Driver>,
    pub(super) geometry: Geometry,
    pub(super) producer: Option<Arc<Dependencies>>,
    pub(super) owner: Option<MasterRef<Driver>>,
    pub(super) kind: Kind,
    pub(super) zpos: u32,
}

pub(super) const MAX_PLANES: usize = 24;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Primary,
    Overlay,
    Cursor,
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
    /// Retain the exact atomic backend, independently of subsequent offer withdrawal.
    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) fn set_constraints(&mut self, entry: Option<&OpaqueEntry>) {
        self.constraints = entry.map(ARef::from);
        self.binding = None;
    }

    pub(super) fn set_binding(&mut self,
        entry: Option<&Binding>,
    ) {
        self.constraints = entry.map(|entry| ARef::from(&**entry));
        self.binding = entry.cloned();
    }

    /// Unknown or delegated native bindings are never permission for a HOST fallback.
    pub(crate) fn host_binding(&self) -> bool {
        match &self.binding {
            Some(entry) => matches!(&*entry.backend(),
                crate::execution::constraints::backend::Backend::Host),
            None => self.constraints.is_none(),
        }
    }

    pub(super) fn constraints(&self) -> Option<&OpaqueEntry> {
        self.constraints.as_deref()
    }

    pub(crate) fn renderer_worker(&self) -> Result<Arc<crate::renderer::ready::Worker>> {
        match self.binding.as_ref().map(Binding::backend) {
            Some(crate::execution::constraints::backend::BackendRef::Renderer(backend)) => {
                match &*backend {
                    crate::execution::constraints::backend::Backend::Renderer(worker) => {
                        Ok(worker.clone())
                    }
                    _ => Err(EOPNOTSUPP),
                }
            }
            _ => Err(EOPNOTSUPP),
        }
    }

    /// Compare backing reservations without mapping pixels or acquiring a source read.
    pub(super) fn uses_reservation(&self, reservation: &Reservation) -> Result<bool> {
        for primary in self.layers() {
            for plane in 0..primary.framebuffer.plane_count() {
                if core::ptr::eq(
                    primary.framebuffer.object_at(plane)?.reservation(),
                    reservation,
                ) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) fn primary(&self) -> Option<&Primary> {
        self.layers().find(|layer| layer.kind == Kind::Primary)
    }

    pub(super) fn layers(&self) -> impl Iterator<Item = &Primary> {
        self.layers.iter().filter_map(|layer| layer.as_deref())
    }

    pub(super) fn set_layer(&mut self, index: usize, layer: Option<Arc<Primary>>) {
        self.layers[index] = layer;
    }

    /// A composed image is attributable only when every contributing plane agrees.
    pub(super) fn finalize(&mut self, content: Option<ContentSerial>) {
        let owner = self.layers().next().and_then(|layer| layer.owner.clone());
        if self.layers().next().is_some() {
            self.owner = if self.layers().all(|layer| layer.owner == owner) {
                owner
            } else {
                None
            };
            self.content = content;
        } else {
            self.content = None;
        }
    }

    /// Keep terminal native errors distinct from unfinished producer work.
    pub(super) fn producer_state(&self) -> kernel::dma_fence::Status {
        use kernel::dma_fence::Status;

        let mut pending = false;
        for records in self.layers().filter_map(|layer| layer.producer.as_ref()) {
            for fence in records.iter() {
                match fence.status() {
                    Status::Pending => pending = true,
                    Status::Complete(Err(error)) => return Status::Complete(Err(error)),
                    Status::Complete(Ok(())) => (),
                }
            }
        }
        if pending {
            Status::Pending
        } else {
            Status::Complete(Ok(()))
        }
    }

    pub(super) fn producer_result(&self) -> Result {
        match self.producer_state() {
            kernel::dma_fence::Status::Pending => Err(EAGAIN),
            kernel::dma_fence::Status::Complete(result) => result,
        }
    }

    /// Retain one native wait for the producer records acquired with this scene.
    pub(super) fn producer_completion(&self) -> Result<Option<ARef<kernel::dma_fence::Fence>>> {
        let mut records = KVec::new();
        for dependencies in self.layers().filter_map(|layer| layer.producer.as_ref()) {
            for fence in dependencies.iter() {
                records.push(fence.to_owned_ref(), GFP_KERNEL)?;
            }
        }
        kernel::dma_fence::Fence::merge_completion(&records)
    }

    /// Blank output has no framebuffer content revision, not an unchanged revision.
    pub(super) fn content_serial(&self) -> Option<ContentSerial> {
        self.content
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) fn producer_failed(&self) -> bool {
        self.layers()
            .filter_map(|primary| primary.producer.as_ref())
            .any(|records| {
                records.iter().any(|fence| {
                    matches!(fence.status(), kernel::dma_fence::Status::Complete(Err(_)))
                })
            })
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) fn producer_status(&self) -> Option<kernel::dma_fence::Status> {
        self.primary()?
            .producer
            .as_ref()?
            .iter()
            .next()
            .map(|fence| fence.status())
    }

    pub(super) fn owner(&self) -> Option<&MasterRef<Driver>> {
        self.owner.as_ref()
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) fn new(
        framebuffer: FramebufferRef<Driver>,
        geometry: Geometry,
        content: ContentSerial,
        owner: Option<MasterRef<Driver>>,
        producer: Option<Arc<Dependencies>>,
    ) -> Result<Self> {
        let mut scene = Self::blank(owner.clone());
        scene.layers[0] = Some(Arc::new(
            Primary {
                yuv: (
                    kernel::drm::kms::plane::ColorEncoding::Bt601,
                    kernel::drm::kms::plane::ColorRange::Limited,
                ),
                color: None,
                framebuffer,
                geometry,
                producer,
                owner,
                kind: Kind::Primary,
                zpos: 0,
            },
            GFP_KERNEL,
        )?);
        scene.content = Some(content);
        Ok(scene)
    }

    pub(super) fn blank(owner: Option<MasterRef<Driver>>) -> Self {
        Self {
            output_color: None,
            layers: core::array::from_fn(|_| None),
            content: None,
            owner,
            constraints: None,
            binding: None,
        }
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
