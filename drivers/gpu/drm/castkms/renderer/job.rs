// SPDX-License-Identifier: GPL-2.0-only

//! Claimed scene reads, independent of renderer file transport and native submission.

use crate::{display_control, scene::Scene};
use kernel::{dma_fence::Fence, drm::preparation::ReadClaim, prelude::*};

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
    pub(super) fn claim(current: &display_control::Current<'_>) -> Result<Self> {
        let (scene, claim) = current.claim_scene()?;
        Ok(Self { scene, claim })
    }

    /// Borrow retained scene metadata without transferring its source-read claim.
    pub(crate) fn scene(&self) -> &Scene {
        &self.scene
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
}
