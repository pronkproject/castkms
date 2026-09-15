// SPDX-License-Identifier: GPL-2.0-only

//! Claimed scene reads, independent of renderer file transport and native submission.

use crate::{display_control, scene::Scene};
use kernel::{
    dma_buf::DmaBuf,
    dma_fence::Fence,
    drm::{
        gem::{BaseObject, ExportAccess},
        preparation::ReadClaim, //
    },
    prelude::*,
    sync::aref::ARef, //
};

/// Immutable metadata for one claimed source scene.
#[derive(Clone, Copy)]
pub(crate) struct Description {
    pub(crate) format: u32,
    pub(crate) modifier: Option<u64>,
    pub(crate) dimensions: [u32; 2],
    pub(crate) source: [u32; 4],
    pub(crate) destination: [u32; 2],
    pub(crate) output: [u32; 2],
    pub(crate) plane_count: usize,
    pub(crate) content_serial: u64,
}

/// One framebuffer plane borrowed from a claimed source job.
pub(crate) struct Plane<'a> {
    object: &'a <crate::Driver as kernel::drm::Driver>::Object,
    pub(crate) pitch: u32,
    pub(crate) offset: u32,
}

impl Plane<'_> {
    /// Retain this plane as an ordinary DMA-BUF without installing a descriptor.
    pub(crate) fn export(&self) -> Result<ARef<DmaBuf>> {
        match self.object.imported_dma_buf() {
            Some(buffer) => Ok(buffer),
            None => self.object.export_dma_buf(ExportAccess::ReadOnly),
        }
    }
}

/// One authorized read of an owned scene description.
///
/// Scene retention keeps its storage alive. The read claim separately prevents source
/// retirement until one consuming release operation records how access ended. Dropping an
/// unresolved job reports terminal service failure through the preparation accounting.
#[must_use = "a source job must be released after access"]
pub(crate) struct SourceJob {
    scene: Scene,
    claim: ReadClaim,
}

impl SourceJob {
    /// Claim and retain the currently authorized scene as one indivisible operation.
    pub(super) fn claim(
        current: &display_control::Current<'_>,
        previous_content_serial: Option<u64>,
    ) -> Result<Self> {
        let (scene, claim) = current.claim_changed_scene(previous_content_serial)?;
        Ok(Self { scene, claim })
    }

    /// Borrow retained scene metadata without transferring its source-read claim.
    pub(crate) fn scene(&self) -> &Scene {
        &self.scene
    }

    /// Describe the claimed source without exporting storage or changing its lifetime.
    pub(crate) fn description(&self) -> Result<Description> {
        let primary = self.scene.primary().ok_or(ENODATA)?;
        let framebuffer = primary.framebuffer();
        let geometry = primary.geometry();
        Ok(Description {
            format: framebuffer.format(),
            modifier: framebuffer.modifier(),
            dimensions: [framebuffer.width(), framebuffer.height()],
            source: geometry.source,
            destination: geometry.destination,
            output: geometry.output,
            plane_count: framebuffer.plane_count(),
            content_serial: self.scene.content_serial().ok_or(ENODATA)?.get(),
        })
    }

    /// Borrow one described source plane for preparation outside authorization locks.
    pub(crate) fn plane(&self, index: usize) -> Result<Plane<'_>> {
        let framebuffer = self.scene.primary().ok_or(ENODATA)?.framebuffer();
        Ok(Plane {
            object: framebuffer.object_at(index)?,
            pitch: framebuffer.pitch(index)?,
            offset: framebuffer.offset(index)?,
        })
    }

    /// Promise that no source access occurred under this job.
    pub(crate) fn release_without_access(self) {
        self.claim.release_cpu();
    }

    /// Promise that all synchronous CPU source access has ended.
    pub(crate) fn release_cpu(self) {
        self.claim.release_cpu();
    }

    /// Promise that no further work will be submitted and transfer native completion.
    pub(crate) fn release_submitted(self, fence: &Fence) {
        self.claim.release_submitted(fence);
    }

    pub(crate) fn release(self, completion: Completion) {
        match completion {
            Completion::WithoutAccess => self.release_without_access(),
            Completion::Cpu => self.release_cpu(),
            Completion::Submitted(fence) => self.release_submitted(&fence),
        }
    }
}

/// Terminal account of access performed under one published job.
pub(crate) enum Completion {
    WithoutAccess,
    Cpu,
    Submitted(ARef<Fence>),
}
