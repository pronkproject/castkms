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
    time::{Delta, Instant, Monotonic},
};

/// Evidence captured with an authorized source claim, not a capture grant or private image.
///
/// Producer records and exact backend attribution are retained from the scene. This object
/// cannot delay source retirement by retaining a read claim or a framebuffer reference.
pub(super) struct Evidence {
    output: Identity,
    configuration: Configuration,
    owner: Option<MasterRef<Driver>>,
    execution: Description,
    constraints: Option<ARef<kernel::drm::constraints::OpaqueEntry>>,
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
            constraints: scene.constraints().map(ARef::from),
            content: scene.content_serial(),
            producers,
        }
    }

    pub(super) fn release(self, completion: Option<ARef<Fence>>) -> Released {
        Released {
            evidence: self,
            cpu_completed_at: completion.is_none().then(Instant::<Monotonic>::now),
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
    cpu_completed_at: Option<Instant<Monotonic>>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Released {
    fn fences(&self) -> impl Iterator<Item = &Fence> {
        Iterator::chain(
            self.evidence
                .producers
                .iter()
                .flatten()
                .flat_map(|producer| producer.iter()),
            self.completion.iter().map(|fence| &**fence),
        )
    }

    /// Pixel validity only. An error is not proof that every native access has stopped.
    pub(crate) fn status(&self) -> Status {
        let mut pending = false;
        for fence in self.fences() {
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

    /// Original production time, never the time of a recipient copy or dequeue.
    /// CPU reports are timestamped at release; native work uses its concrete fence times.
    /// All original producer records must have succeeded before metadata is available.
    pub(crate) fn completed_at(&self) -> Result<Instant<Monotonic>> {
        match self.status() {
            Status::Pending => return Err(EAGAIN),
            Status::Complete(result) => result?,
        }
        let mut completed = self.cpu_completed_at;
        for fence in self.fences() {
            let signaled = fence.signal_time()?.ok_or(EAGAIN)?;
            if completed.is_none_or(|previous| signaled - previous > Delta::from_nanos(0)) {
                completed = Some(signaled);
            }
        }
        completed.ok_or(EIO)
    }

    pub(crate) fn content_serial(&self) -> Option<ContentSerial> {
        self.evidence.content
    }

    /// Observe eligibility under the caller's current display and renderer exclusion.
    /// Success grants no capture authority and reserves no later operation.
    pub(crate) fn check(&self, current: &Current<'_>, execution: Description) -> Result {
        current.check_scene_owner()?;
        if self.evidence.output != *current.output_identity()
            || self.evidence.owner.as_ref() != Some(current.master())
        {
            return Err(EACCES);
        }
        current.check_constraints(self.evidence.constraints.as_deref())?;
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
