// SPDX-License-Identifier: GPL-2.0-only

//! Endpoint-owned native renderer offers, published only after reply preparation.

use super::{draft::Draft, permission::Access, private_pool::Pool, ready};
use crate::{execution::constraints::backend::Binding, Driver};
use kernel::{drm::{device::Registered, Device}, prelude::*};

/// A unique endpoint lifetime with an immutable native identity and pinned private pool.
/// Retaining an entry alone cannot keep its worker ready after this owner is dropped.
/// Construct and drop outside DRM and provider locks; endpoint serialization must exclude
/// close throughout publication and install the owner without a later fallible operation.
pub(crate) struct Offer {
    access: Access,
    entry: Binding,
    _owner: ready::Owner,
}

impl Offer {
    /// Prepare without listing, selecting, or requiring an enabled output.
    pub(crate) fn new(
        registered: &Device<Driver, Registered>,
        draft: &Draft,
        pool: &Pool,
    ) -> Result<Self> {
        let access = draft.access();
        access.with_output(|| Ok(()))?;
        let output = access.constraints_output(registered)?;
        let provider = access.display().constraints.as_ref().ok_or(EOPNOTSUPP)?;
        // Cleanup can drop backend/device references, so it precedes all authority locks.
        provider.reap(&output)?;
        let owner = draft.prepare_worker(pool)?;
        let entry = provider.prepare(owner.worker())?;
        access.with_output(|| Ok(()))?;
        Ok(Self { access: access.clone(), entry, _owner: owner })
    }

    pub(crate) fn entry(&self) -> &Binding {
        &self.entry
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
        self.access.with_output(|| {
            reply(self.entry.id())?;
            provider.publish(&output, &self.entry)
        })
    }
}
