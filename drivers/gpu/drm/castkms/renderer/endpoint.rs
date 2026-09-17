// SPDX-License-Identifier: GPL-2.0-only

//! Serialized preparation and publication of one immutable renderer endpoint.

mod stream;

use super::{draft::Draft, offer::Offer, permission::Access, private_pool::Pool};
use crate::{execution::capabilities::Profile, Driver};
use kernel::{
    dma_buf::DmaBuf,
    dma_fence::Fence,
    drm::device::RegisteredDeviceRef,
    prelude::*,
    sync::{aref::ARef, Arc, Mutex},
};

enum State {
    Empty,
    Draft { draft: Arc<Draft>, pool: Pool },
    Publishing,
    Ready { offer: Offer, pool: Pool, source: stream::Stream },
    Closed,
}

/// One worker declaration per file lifetime. Replacement uses an independent endpoint.
/// A retained endpoint reference is not permission to renew a closed worker.
#[pin_data(PinnedDrop)]
pub(crate) struct Endpoint {
    access: Access,
    device: RegisteredDeviceRef<Driver>,
    #[pin]
    state: Mutex<State>,
}

impl Endpoint {
    pub(crate) fn new(access: Access, device: RegisteredDeviceRef<Driver>) -> Result<Arc<Self>> {
        Arc::pin_init(pin_init!(Self {
            access,
            device,
            state <- kernel::new_mutex!(State::Empty),
        }), GFP_KERNEL)
    }

    /// Allocate an immutable declaration without changing native constraints availability.
    /// Failed preparation leaves an empty endpoint available for retry.
    pub(crate) fn declare(&self, profile: Profile, dimensions: [u32; 2]) -> Result {
        let draft = Arc::new(Draft::new(self.access.clone(), profile, dimensions)?, GFP_KERNEL)?;
        let pool = Pool::new()?;
        let mut state = self.state.lock();
        match &*state {
            State::Empty => (),
            State::Closed => return Err(EKEYREVOKED),
            _ => return Err(EALREADY),
        }
        self.access.with_output(|| Ok(()))?;
        *state = State::Draft { draft, pool };
        Ok(())
    }

    pub(crate) fn register_image(
        &self,
        id: u64,
        dimensions: [u32; 2],
        buffers: &[ARef<DmaBuf>],
    ) -> Result {
        let mut state = self.state.lock();
        match &mut *state {
            State::Draft { draft, pool } => {
                if dimensions != draft.dimensions() {
                    return Err(EINVAL);
                }
                pool.insert(id, || draft.register_image(buffers))
            }
            State::Closed => Err(EKEYREVOKED),
            State::Empty => Err(ENODATA),
            State::Publishing | State::Ready { .. } => Err(EBUSY),
        }
    }

    pub(crate) fn unregister_image(&self, id: u64) -> Result {
        let retired = {
            let mut state = self.state.lock();
            match &mut *state {
                State::Draft { pool, .. } => pool.remove(id)?,
                State::Ready { pool, source, .. } => {
                    if source.references(id) {
                        return Err(EBUSY);
                    }
                    pool.remove(id)?
                }
                State::Closed => return Err(EKEYREVOKED),
                State::Empty => return Err(ENODATA),
                State::Publishing => return Err(EBUSY),
            }
        };
        drop(retired);
        Ok(())
    }

    pub(crate) fn submit_probe(&self, completion: Option<ARef<Fence>>) -> Result {
        let draft = {
            let state = self.state.lock();
            match &*state {
                State::Draft { draft, .. } => draft.clone(),
                State::Closed => return Err(EKEYREVOKED),
                State::Empty => return Err(ENODATA),
                State::Publishing | State::Ready { .. } => return Err(EBUSY),
            }
        };
        draft.submit_probe(completion)
    }

    /// Prepare outside endpoint exclusion, then serialize reply, listing and owner install.
    /// No fallible operation follows successful listing. The reply callback follows Offer's
    /// restrictions; closing concurrently cannot leave a ready unowned native entry.
    pub(crate) fn publish(&self, reply: impl FnOnce(u64) -> Result) -> Result {
        let resources = {
            let mut state = self.state.lock();
            match &*state {
                State::Draft { .. } => (),
                State::Closed => return Err(EKEYREVOKED),
                State::Empty => return Err(ENODATA),
                State::Publishing => return Err(EBUSY),
                State::Ready { .. } => return Err(EALREADY),
            }
            core::mem::replace(&mut *state, State::Publishing)
        };
        let mut pending = Pending { endpoint: self, resources: Some(resources) };
        let registered = self.device.registration_guard().ok_or(ENODEV)?;
        let Some(State::Draft { draft, pool }) = &pending.resources else {
            return Err(EIO);
        };
        // This owner is declared before the lock, so errors revoke it after lock release.
        let offer = Offer::new(&registered, draft, pool)?;
        let mut state = self.state.lock();
        if matches!(&*state, State::Closed) {
            return Err(EKEYREVOKED);
        }
        if !matches!(&*state, State::Publishing) {
            return Err(ECANCELED);
        }
        // Extract all fallible bookkeeping before making the offer selectable.
        let Some(State::Draft { draft, pool }) = pending.resources.take() else {
            return Err(EIO);
        };
        let result = offer.publish(&registered, reply);
        match result {
            Ok(()) => *state = State::Ready { offer, pool, source: stream::Stream::new() },
            Err(error) => {
                drop(state);
                pending.resources = Some(State::Draft { draft, pool });
                return Err(error);
            }
        }
        drop(state);
        drop(draft);
        Ok(())
    }

    /// Advisory identity only, not a promise that an entry is selected or still ready.
    pub(crate) fn constraints_id(&self) -> Result<u64> {
        let state = self.state.lock();
        match &*state {
            State::Closed => Err(EKEYREVOKED),
            State::Ready { offer, .. } => self.access.with_output(|| Ok(offer.entry().id())),
            _ => self.access.with_output(|| Ok(0)),
        }
    }

    /// Exclude new operations before revocation or resource destruction outside the mutex.
    pub(crate) fn close(&self) {
        let retired = core::mem::replace(&mut *self.state.lock(), State::Closed);
        drop(retired);
        self.access.device().changed.notify_all();
    }
}

#[pinned_drop]
impl PinnedDrop for Endpoint {
    fn drop(self: Pin<&mut Self>) {
        self.close();
    }
}

/// Restore failed preparation only if the endpoint has not closed in the meantime.
struct Pending<'a> {
    endpoint: &'a Endpoint,
    resources: Option<State>,
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        let mut state = self.endpoint.state.lock();
        if matches!(&*state, State::Publishing) {
            if let Some(resources) = self.resources.take() {
                *state = resources;
            }
        }
        // Retained resources in self are destroyed after this guard is released.
    }
}
