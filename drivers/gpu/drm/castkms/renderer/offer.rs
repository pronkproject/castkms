// SPDX-License-Identifier: GPL-2.0-only

//! Endpoint-owned native renderer offers, published only after reply preparation.

use super::{draft::Draft, permission::Access, private_pool::Pool, ready};
use crate::{execution::constraints::backend::Backend, Driver};
use kernel::{dma_fence::Fence, drm::{device::Registered, Device}, prelude::*};

type Binding = kernel::sync::aref::ARef<kernel::drm::constraints::Entry<Backend>>;

/// A unique endpoint lifetime with an immutable native identity and pinned private pool.
/// Retaining an entry alone cannot keep its worker ready after this owner is dropped.
/// Construct and drop outside DRM and provider locks; endpoint serialization must exclude
/// close throughout publication and install the owner without a later fallible operation.
pub(crate) struct Offer {
    access: Access,
    entry: Binding,
    list: kernel::sync::aref::ARef<kernel::drm::constraints::List>,
    _owner: ready::Owner,
}

impl Drop for Offer {
    fn drop(&mut self) {
        self._owner.revocation().revoke();
        // The endpoint is gone, so no new resolution should retain its callback module.
        // Accepted states and source jobs keep their own exact typed binding.
        if let Some(provider) = self.access.display().constraints.as_ref() {
            drop(provider.remove(&self.entry));
        }
        // Selected entries remain natively retained until KMS replacement/recovery.
        let _ = self.list.forget(self.entry.id());
    }
}

impl Offer {
    /// Prepare without listing, selecting, or requiring an enabled output.
    pub(crate) fn new(
        registered: &Device<Driver, Registered>,
        draft: &Draft,
        pool: &Pool,
        completion: Option<&Fence>,
    ) -> Result<Self> {
        let access = draft.access();
        access.with_output_interval(draft.interval(), || Ok(()))?;
        let output = access.constraints_output(registered)?;
        let provider = access.display().constraints.as_ref().ok_or(EOPNOTSUPP)?;
        // Cleanup can drop backend/device references, so it precedes all authority locks.
        provider.reap(&output)?;
        let owner = draft.prepare_worker(pool, completion)?;
        let entry = provider.prepare(owner.worker())?;
        access.with_output_interval(draft.interval(), || Ok(()))?;
        Ok(Self {
            access: access.clone(), entry, _owner: owner,
            list: kernel::sync::aref::ARef::from(output.list()),
        })
    }

    pub(crate) fn entry(&self) -> &Binding {
        &self.entry
    }

    /// Retain admission metadata without extending the unique worker owner lifetime.
    pub(crate) fn control(&self) -> Control {
        Control { access: self.access.clone(), entry: self.entry.clone() }
    }

    /// Retain only withdrawal authority for cleanup outside endpoint exclusion.
    pub(super) fn revocation(
        &self,
    ) -> kernel::sync::aref::ARef<kernel::drm::capture::Revocation> {
        self._owner.revocation()
    }

    pub(super) fn is_live(&self) -> bool {
        self._owner.worker().is_live()
    }

    pub(super) fn interval(&self) -> crate::authority::Interval {
        self._owner.worker().interval()
    }

    /// Copy the reply before native publication while issuer authority is stable.
    /// On any error callers ignore the copied identity. The callback must not publish files,
    /// select this entry, revoke its owner, or reenter authority. Success lists a ready offer
    /// but does not change accepted state; the endpoint must retain this owner before unlock.
    pub(crate) fn publish(
        &self,
        registered: &Device<Driver, Registered>,
        reply: impl FnOnce(u64) -> Result,
    ) -> Result {
        let output = self.access.constraints_output(registered)?;
        let provider = self.access.display().constraints.as_ref().ok_or(EOPNOTSUPP)?;
        self.access.with_output_interval(self.interval(), || {
            reply(self.entry.id())?;
            provider.publish(&output, &self.entry)
        })
    }
}

/// Exact offer admission, revocable independently of every retained clone.
#[derive(Clone)]
pub(crate) struct Control {
    access: Access,
    entry: Binding,
}

impl Control {
    pub(crate) fn entry(&self) -> &Binding {
        &self.entry
    }

    pub(crate) fn worker(&self) -> Result<kernel::sync::Arc<ready::Worker>> {
        let backend = self.entry.backend();
        let Backend::Renderer(worker) = &*backend else {
            return Err(EOPNOTSUPP);
        };
        Ok(worker.clone())
    }

    /// Stabilize the accepted entry and live worker without acquiring a source read.
    /// Callbacks may not wait, revoke, reenter admission, or release native resources.
    pub(crate) fn with_current<R>(
        &self,
        f: impl FnOnce(&crate::display_control::Current<'_>) -> Result<R>,
    ) -> Result<R> {
        let backend = self.entry.backend();
        let crate::execution::constraints::backend::Backend::Renderer(worker) = &*backend else {
            return Err(EINVAL);
        };
        self.access.with_current(|current| {
            if worker.interval() != self.access.device().authority.interval()? {
                return Err(ESTALE);
            }
            let _ready = worker.hold_ready()?;
            current.check_constraints(Some(&self.entry))?;
            f(&current)
        })
    }

    pub(crate) fn claim(
        &self,
        image: u64,
        previous: Option<u64>,
        destination: super::private_image::Prepared,
    ) -> Result<super::render_job::RenderJob> {
        super::render_job::RenderJob::claim_bound(
            &self.access, &self.entry, image, previous, destination,
        )
    }

    /// Recheck admission after reply encoding, immediately before installing source files.
    /// A failed publication must release the unpublished job without access outside locks.
    pub(crate) fn publish_source(
        &self,
        job: &super::render_job::RenderJob,
        publish: impl FnOnce(),
    ) -> Result {
        if !job.source().scene().constraints()
            .is_some_and(|entry| core::ptr::eq(entry, &**self.entry))
        {
            return Err(EACCES);
        }
        self.with_current(|_| { publish(); Ok(()) })
    }

    pub(crate) fn check_completed(&self, rendered: &super::render_job::Rendered) -> Result {
        self.with_current(|current| rendered.content().check_bound(current))
    }
}
