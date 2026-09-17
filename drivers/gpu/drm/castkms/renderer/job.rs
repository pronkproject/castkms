// SPDX-License-Identifier: GPL-2.0-only

//! Claimed scene reads, independent of renderer file transport and native submission.

mod binding;

use super::content::{Evidence, Released};
use crate::{display_control, execution::Description, scene::Scene};
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

/// One framebuffer plane borrowed from a claimed source job.
pub(crate) struct Plane<'a> {
    object: &'a <crate::Driver as kernel::drm::Driver>::Object,
    pub(crate) pitch: u32,
    pub(crate) offset: u32,
}

impl Plane<'_> {
    pub(super) fn new(layer: &crate::scene::Primary, index: usize) -> Result<Plane<'_>> {
        let framebuffer = layer.framebuffer();
        Ok(Plane {
            object: framebuffer.object_at(index)?,
            pitch: framebuffer.pitch(index)?,
            offset: framebuffer.offset(index)?,
        })
    }

    pub(crate) fn shares_storage_with(&self, other: &Self) -> bool {
        core::ptr::eq(self.object, other.object)
    }

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
    evidence: Evidence,
    binding: binding::Retained,
}

impl SourceJob {
    /// Claim and retain the currently authorized scene as one indivisible operation.
    pub(super) fn claim(
        current: &display_control::Current<'_>,
        previous_content_serial: Option<u64>,
        execution: Description,
    ) -> Result<Self> {
        current.check_constraints(None)?;
        Self::claim_current(current, previous_content_serial, Some(execution))
    }

    /// Admit a source read only for the accepted entry and its held ready worker.
    /// The caller holds current permission exclusion before acquiring readiness.
    /// Retaining an entry or observing list selection alone cannot authorize this call.
    pub(crate) fn claim_bound(
        current: &display_control::Current<'_>,
        entry: &kernel::drm::constraints::Entry<crate::execution::constraints::backend::Backend>,
        ready: &super::ready::Ready<'_>,
        previous_content_serial: Option<u64>,
    ) -> Result<Self> {
        let backend = entry.backend();
        let crate::execution::constraints::backend::Backend::Renderer(worker) = &*backend else {
            return Err(EOPNOTSUPP);
        };
        if !ready.belongs_to(worker) || worker.output() != current.output_identity() {
            return Err(EACCES);
        }
        current.check_constraints(Some(entry))?;
        Self::claim_current(current, previous_content_serial, None)
    }

    fn claim_current(
        current: &display_control::Current<'_>,
        previous_content_serial: Option<u64>,
        execution: Option<Description>,
    ) -> Result<Self> {
        let (scene, claim) = current.claim_changed_scene(previous_content_serial)?;
        let binding = match binding::Retained::new(scene.constraints()) {
            Ok(binding) => binding,
            Err(error) => {
                // The claim has not escaped, so no renderer access has been admitted.
                claim.release_cpu();
                return Err(error);
            }
        };
        let evidence = Evidence::new(current, &scene, execution);
        Ok(Self {
            scene,
            claim,
            evidence,
            binding,
        })
    }

    /// Borrow retained scene metadata without transferring its source-read claim.
    pub(crate) fn scene(&self) -> &Scene {
        &self.scene
    }

    /// Retain the producer wait captured when KMS accepted this scene.
    pub(crate) fn producer_completion(&self) -> Result<Option<ARef<Fence>>> {
        let completion = self.scene.producer_completion()?;
        match self.scene.producer_state() {
            kernel::dma_fence::Status::Complete(Ok(())) => Ok(None),
            kernel::dma_fence::Status::Pending => Ok(completion),
            // Native status remains on the producer fence. Do not expose its errno
            // as protocol readiness (for example EAGAIN, EBUSY or ENODATA).
            kernel::dma_fence::Status::Complete(Err(_)) => Err(EREMOTEIO),
        }
    }

    /// Promise that no source access occurred under this job.
    pub(crate) fn release_without_access(self) {
        self.binding.finish(None);
        self.claim.release_cpu();
    }

    /// Promise that all synchronous CPU source access has ended.
    pub(crate) fn release_cpu(self) -> Released {
        self.binding.finish(None);
        self.claim.release_cpu();
        self.evidence.release(None)
    }

    /// Promise that no further work will be submitted and transfer native completion.
    pub(crate) fn release_submitted(self, fence: ARef<Fence>) -> Released {
        self.binding.finish(Some(&fence));
        self.claim.release_submitted(&fence);
        self.evidence.release(Some(fence))
    }

    pub(crate) fn release(self, completion: Completion) -> Option<Released> {
        match completion {
            Completion::WithoutAccess => {
                self.release_without_access();
                None
            }
            Completion::Cpu => Some(self.release_cpu()),
            Completion::Submitted(fence) => Some(self.release_submitted(fence)),
        }
    }
}

/// Terminal account of access performed under one published job.
pub(crate) enum Completion {
    WithoutAccess,
    Cpu,
    Submitted(ARef<Fence>),
}
