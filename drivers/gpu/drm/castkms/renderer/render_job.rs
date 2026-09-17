// SPDX-License-Identifier: GPL-2.0-only

//! An authorized source read bound to independently reserved private storage.

use super::{
    content::Released,
    job::{Completion, SourceJob},
    private_image::{Access, Image, Prepared, Use},
};
use kernel::{
    dma_fence::wakeup::Wakeup,
    prelude::*,
    sync::{poll::PollCondVar, Arc},
};

const MAX_PRODUCER_OBSERVATIONS: usize = 1024;

/// One bounded source-to-private-image stage, not an output write to a recipient.
///
/// The renderer must isolate its native source queue and mappings from downstream waits.
/// Reserving storage here does not prove that an external GPU submission obeys that rule.
#[must_use = "a render job must report how all source and private-image access ended"]
pub(crate) struct RenderJob {
    source: SourceJob,
    destination: Access,
    producer_observations: KVec<Wakeup>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl RenderJob {
    /// Bind a prepared private image to the exact accepted worker under live authority.
    /// Storage reservation happens before entering native and worker admission locks.
    pub(crate) fn claim_bound(
        access: &super::permission::Access,
        entry: &kernel::drm::constraints::Entry<crate::execution::constraints::backend::Backend>,
        image_id: u64,
        previous_content_serial: Option<u64>,
        destination: Prepared,
    ) -> Result<Self> {
        let backend = entry.backend();
        let crate::execution::constraints::backend::Backend::Renderer(worker) = &*backend else {
            return Err(EOPNOTSUPP);
        };
        if worker.interval() != access.interval() {
            return Err(ESTALE);
        }
        let source = access.with_current(|current| {
            let ready = worker.hold_ready()?;
            if !ready.contains(image_id, destination.image()) {
                return Err(EACCES);
            }
            if destination.image().dimensions() != current.configuration().dimensions() {
                return Err(ESTALE);
            }
            for buffer in destination.image().buffers() {
                if current.uses_reservation(buffer.reservation())? {
                    return Err(EINVAL);
                }
            }
            SourceJob::claim_bound(&current, entry, &ready, previous_content_serial)
        })?;
        Ok(Self::new(source, destination))
    }

    pub(super) fn new(source: SourceJob, destination: Prepared) -> Self {
        Self {
            source,
            destination: destination.claim(),
            producer_observations: KVec::new(),
        }
    }

    pub(crate) fn source(&self) -> &SourceJob {
        &self.source
    }

    pub(crate) fn destination(&self) -> &Image {
        self.destination.image()
    }

    /// Arrange bounded producer readiness notifications before publishing access to a worker.
    /// Failure grants no new access and must be followed by the caller's no-access release.
    /// Observations follow completed content after source release and detach on its removal.
    pub(crate) fn observe_producers(&mut self, changed: &Arc<PollCondVar>) -> Result {
        let mut observations: KVec<Wakeup> = KVec::new();
        for fence in self
            .source
            .scene()
            .layers()
            .filter_map(|layer| layer.producer.as_ref())
            .flat_map(|producer| producer.iter())
        {
            if observations
                .iter()
                .any(|observation| core::ptr::eq(observation.fence(), fence))
            {
                continue;
            }
            if observations.len() == MAX_PRODUCER_OBSERVATIONS {
                return Err(E2BIG);
            }
            observations.push(Wakeup::new(fence, changed.clone())?, GFP_KERNEL)?;
        }
        self.producer_observations = observations;
        Ok(())
    }

    /// End the complete stage. Submitted completion must cover source reads and private
    /// writes. No-access promises neither occurred; CPU completion includes coherency.
    pub(crate) fn release(self, completion: Completion) -> Option<Rendered> {
        let fence = match &completion {
            Completion::Submitted(fence) => Some(&**fence),
            Completion::Cpu | Completion::WithoutAccess => None,
        };
        let usage = self.destination.finish(fence);
        self.source.release(completion).map(|content| Rendered {
            usage,
            content,
            _producer_observations: self.producer_observations,
        })
    }
}

/// Private storage held with the original source-stage evidence, not a capture grant.
///
/// Storage remains unavailable for overwrite while this record or a native stage retains
/// its use. Dropping it does not end a still-pending native write. No source claim survives.
pub(crate) struct Rendered {
    usage: Arc<Use>,
    content: Released,
    _producer_observations: KVec<Wakeup>,
}

impl Rendered {
    pub(crate) fn image(&self) -> &Image {
        self.usage.image()
    }

    pub(crate) fn content(&self) -> &Released {
        &self.content
    }
}
