// SPDX-License-Identifier: GPL-2.0-only

//! Exact native entries binding allocation descriptions to prepared execution resources.

use super::Topology;
use crate::{
    execution::validation::{
        Contract,
        SceneView, //
    },
    output::Identity,
    renderer::ready::{
        Ready,
        Worker, //
    }, //
};
use kernel::{
    drm::constraints::{
        Domain,
        Entry, //
    },
    prelude::*,
    sync::{
        aref::ARef,
        Arc, //
    }, //
};

/// No mutable current-renderer lookup is needed to interpret a retained entry.
pub(crate) enum Backend {
    Host,
    Renderer(Arc<Worker>),
}

pub(crate) type Binding = ARef<Entry<Backend>>;

// SAFETY: The CastKMS module retains the enum's code and every owned resource destructor.
#[vtable]
unsafe impl kernel::drm::constraints::Backend for Backend {}

impl Backend {
    /// Construct an unpublished native identity after checking the exact output scope.
    /// Native output attachment/publication separately validates the existing KMS topology.
    pub(crate) fn entry(
        self,
        domain: &Domain,
        crtc: u32,
        output: &Identity,
        topology: &Topology,
    ) -> Result<ARef<Entry<Self>>> {
        let description = match &self {
            Self::Host => super::host(topology.planes(), &[])?,
            Self::Renderer(worker) => {
                if worker.output() != output {
                    return Err(EINVAL);
                }
                super::renderer(worker.profile(), topology.planes(), &[])?
            }
        };
        Entry::new(domain, crtc, &description, Arc::new(self, GFP_KERNEL)?)
    }

    /// Advisory metadata validation, including under a separately held installation guard.
    pub(crate) fn check(
        &self,
        output: &Identity,
        interval: Option<crate::authority::Interval>,
        scene: SceneView<'_>,
    ) -> Result {
        match self {
            Self::Host => Contract::Host.check(scene),
            Self::Renderer(worker) => {
                if worker.output() != output {
                    return Err(EINVAL);
                }
                if Some(worker.interval()) != interval {
                    return Err(ESTALE);
                }
                worker.check_scene(scene)
            }
        }
    }

    /// Serialize final acceptance with renderer loss; HOST owns no delegated worker.
    /// Unchanged, fully disabled shutdown must bypass this operation for failed workers.
    pub(crate) fn hold_ready(&self) -> Result<Option<Ready<'_>>> {
        match self {
            Self::Host => Ok(None),
            Self::Renderer(worker) => worker.hold_ready().map(Some),
        }
    }

    /// Retained native identities survive, but no future work can use the renderer resources.
    /// The caller releases provider/index locks first; native buffers may be destroyed here.
    pub(crate) fn revoke(&self) {
        if let Self::Renderer(worker) = self {
            worker.revoke();
        }
    }
}
