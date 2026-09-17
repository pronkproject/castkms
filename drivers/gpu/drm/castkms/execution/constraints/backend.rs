// SPDX-License-Identifier: GPL-2.0-only

//! Exact native entries binding allocation descriptions to prepared execution resources.

use super::Topology;
use crate::{
    execution::validation::SceneView,
    output::Identity,
    renderer::ready::Worker, //
};
use kernel::{
    drm::constraints::{
        Domain,
        Entry,
        OpaqueEntry, //
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

/// Accepted HOST state needs no provider-owned destructor in its native entry.
/// Renderer entries independently retain their module and private worker resources.
#[derive(Clone)]
pub(crate) enum Binding {
    Host(ARef<OpaqueEntry>),
    Renderer(ARef<Entry<Backend>>),
}

impl Binding {
    pub(crate) fn backend(&self) -> BackendRef<'_> {
        match self {
            Self::Host(_) => BackendRef::Host,
            Self::Renderer(entry) => BackendRef::Renderer(entry.backend()),
        }
    }
}

pub(crate) enum BackendRef<'a> {
    Host,
    Renderer(kernel::sync::ArcBorrow<'a, Backend>),
}

impl core::ops::Deref for BackendRef<'_> {
    type Target = Backend;

    fn deref(&self) -> &Backend {
        match self {
            Self::Host => &Backend::Host,
            Self::Renderer(backend) => backend,
        }
    }
}

impl core::ops::Deref for Binding {
    type Target = OpaqueEntry;

    fn deref(&self) -> &OpaqueEntry {
        match self {
            Self::Host(entry) => entry,
            Self::Renderer(entry) => entry,
        }
    }
}

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
                super::renderer(worker.profile(), topology.planes())?
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
            Self::Host => crate::execution::validation::check_host(scene),
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

    /// Retained native identities survive, but no future work can use the renderer resources.
    /// The caller releases provider/index locks first; native buffers may be destroyed here.
    pub(crate) fn revoke(&self) {
        if let Self::Renderer(worker) = self {
            worker.revoke();
        }
    }
}
