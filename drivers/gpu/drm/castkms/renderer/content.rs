// SPDX-License-Identifier: GPL-2.0-only

//! Source-stage validity and attribution, without retained source storage or read authority.

use crate::{
    display_control::Current,
    execution::Description,
    output::Identity,
    scene::{Configuration, ContentSerial, Scene, MAX_PLANES},
    Driver,
};
use kernel::{
    dma_fence::{Fence, Status},
    drm::{auth::MasterRef, kms::framebuffer::dependencies::Dependencies},
    prelude::*,
    sync::{aref::ARef, Arc},
};

/// Evidence captured with an authorized source claim, not a capture grant or private image.
///
/// Only producer records are retained from the scene. In particular, this object cannot
/// delay source retirement by retaining a read claim or a framebuffer reference.
pub(super) struct Evidence {
    output: Identity,
    configuration: Configuration,
    owner: Option<MasterRef<Driver>>,
    execution: Description,
    content: Option<ContentSerial>,
    producers: [Option<Arc<Dependencies>>; MAX_PLANES],
}

impl Evidence {
    pub(super) fn new(current: &Current<'_>, scene: &Scene, execution: Description) -> Self {
        let mut producers = core::array::from_fn(|_| None);
        for (slot, layer) in producers.iter_mut().zip(scene.layers()) {
            *slot = layer.producer.clone();
        }
        Self {
            output: current.output_identity().clone(),
            configuration: current.configuration().clone(),
            owner: scene.owner().cloned(),
            execution,
            content: scene.content_serial(),
            producers,
        }
    }

    pub(super) fn release(self, completion: Option<ARef<Fence>>) -> Released {
        Released {
            evidence: self,
            completion,
        }
    }
}

/// Renderer-reported source access has ended or has a concrete native completion.
///
/// Successful native completion does not imply successful input production. This record
/// inspects the original producer fences, not a merged wait or a new reservation snapshot.
/// It describes neither an output write nor a registered private allocation. A destination
/// claim must separately bind private storage and live capture authority to these pixels.
pub(crate) struct Released {
    evidence: Evidence,
    completion: Option<ARef<Fence>>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Released {
    /// Pixel validity only. An error is not proof that every native access has stopped.
    pub(crate) fn status(&self) -> Status {
        let mut pending = false;
        let producers = self
            .evidence
            .producers
            .iter()
            .flatten()
            .flat_map(|p| p.iter());
        for fence in Iterator::chain(producers, self.completion.iter().map(|f| &**f)) {
            match fence.status() {
                Status::Pending => pending = true,
                Status::Complete(Err(error)) => return Status::Complete(Err(error)),
                Status::Complete(Ok(())) => (),
            }
        }
        if pending {
            Status::Pending
        } else {
            Status::Complete(Ok(()))
        }
    }

    pub(crate) fn content_serial(&self) -> Option<ContentSerial> {
        self.evidence.content
    }

    /// Observe eligibility under the caller's current display and renderer exclusion.
    /// Success grants no capture authority and reserves no later operation.
    pub(super) fn check(&self, current: &Current<'_>, execution: Description) -> Result {
        current.check_scene_owner()?;
        if self.evidence.output != *current.output_identity()
            || self.evidence.owner.as_ref() != Some(current.master())
        {
            return Err(EACCES);
        }
        if self.evidence.configuration != *current.configuration()
            || self.evidence.execution != execution
        {
            return Err(ESTALE);
        }
        match self.status() {
            Status::Pending => Err(EAGAIN),
            Status::Complete(result) => result,
        }
    }
}
