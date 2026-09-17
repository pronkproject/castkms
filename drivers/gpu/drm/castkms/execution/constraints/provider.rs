// SPDX-License-Identifier: GPL-2.0-only

//! Per-output native entry publication and exact backend resolution.

use super::{
    backend::Backend,
    bindings::Bindings,
    Topology, //
};
use crate::{
    execution::validation::SceneView,
    output::Identity,
    renderer::ready::Worker,
    Driver, //
};
use kernel::{
    drm::{
        constraints::{
            Domain,
            Entry,
            OpaqueEntry, //
        },
        kms::constraints::Output, //
    },
    prelude::*,
    sync::{
        aref::ARef,
        Arc,
        Mutex, //
    }, //
};

pub(crate) const CAPACITY: usize = 64;

/// Driver-owned topology and backend identities, without a retained DRM device reference.
/// The registration owner closes this provider before native KMS shutdown. Closed providers
/// permit no new publication or resolution; native unchanged shutdown retains its old binding.
#[pin_data]
pub(crate) struct Provider {
    domain: ARef<Domain>,
    crtc: u32,
    output: Identity,
    topology: Arc<Topology>,
    initial: ARef<Entry<Backend>>,
    bindings: Arc<Bindings<Backend>>,
    #[pin]
    closed: Mutex<bool>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Provider {
    pub(crate) fn new(
        domain: ARef<Domain>,
        crtc: u32,
        output: Identity,
        topology: Arc<Topology>,
    ) -> Result<Arc<Self>> {
        let initial = Backend::Host.entry(&domain, crtc, &output, &topology)?;
        let bindings = Arc::pin_init(Bindings::new(domain.clone(), crtc, CAPACITY), GFP_KERNEL)?;
        bindings.insert(&initial)?;
        Arc::pin_init(
            pin_init!(Self {
                domain,
                crtc,
                output,
                topology,
                initial,
                bindings,
                closed <- kernel::new_mutex!(false),
            }),
            GFP_KERNEL,
        )
    }

    pub(crate) fn initial(&self) -> &Entry<Backend> {
        &self.initial
    }

    /// Allocate identity and description without listing the worker or changing KMS state.
    pub(crate) fn prepare(&self, worker: Arc<Worker>) -> Result<ARef<Entry<Backend>>> {
        if *self.closed.lock() {
            return Err(ESHUTDOWN);
        }
        Backend::Renderer(worker).entry(&self.domain, self.crtc, &self.output, &self.topology)
    }

    /// Publish only while the exact worker remains ready and this provider remains open.
    /// The caller stabilizes renderer authority and performs any fallible reply copy first.
    /// A failed native add rolls back index membership; neither path selects an entry.
    pub(crate) fn publish(&self, output: &Output<'_, Driver>, entry: &Entry<Backend>) -> Result {
        if !core::ptr::eq(&**self.initial, output.default_entry()) {
            return Err(EINVAL);
        }
        let closed = self.closed.lock();
        if *closed {
            return Err(ESHUTDOWN);
        }
        let backend = entry.backend();
        let Backend::Renderer(worker) = &*backend else {
            return Err(EINVAL);
        };
        if worker.output() != &self.output {
            return Err(EINVAL);
        }
        let mut ready = worker.hold_ready()?;
        self.bindings.insert(entry)?;
        if let Err(error) = ready.publish(output, entry) {
            // The caller still owns entry, so removal cannot destroy its native backend.
            drop(self.bindings.remove(entry));
            return Err(error);
        }
        Ok(())
    }

    /// Exact entry identity, never a cast from arbitrary native provider data.
    pub(crate) fn resolve(&self, entry: &OpaqueEntry) -> Result<ARef<Entry<Backend>>> {
        self.bindings.resolve(entry)
    }

    /// Called under the native list lock, including inside a readiness installation guard.
    /// Neither the publication mutex nor a worker readiness mutex is acquired here.
    pub(crate) fn check(
        &self,
        entry: &OpaqueEntry,
        interval: Option<crate::authority::Interval>,
        scene: SceneView<'_>,
    ) -> Result {
        self.resolve(entry)?
            .backend()
            .check(&self.output, interval, scene)
    }

    /// Withdraw and forget natively first, then remove this index's ownership.
    /// Accepted states and jobs must independently retain their exact entry.
    pub(crate) fn remove(&self, entry: &OpaqueEntry) -> Option<ARef<Entry<Backend>>> {
        self.bindings.remove(entry)
    }

    /// Exclude publication before draining index-owned resources outside provider locks.
    /// Native states can retain terminal worker identities without retaining their pools.
    pub(crate) fn close(&self) {
        *self.closed.lock() = true;
        let entries = self.bindings.close();
        for entry in &entries {
            entry.backend().revoke();
        }
        drop(entries);
    }
}
