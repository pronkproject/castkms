// SPDX-License-Identifier: GPL-2.0-only

//! Serialized preparation and publication of one renderer generation at a time.

mod stream;
mod output;

use super::{
    configuration::Configuration, permission::Access, private_pool::Pool,
    publication::Publication,
};
use crate::{authority::Interval, execution::capabilities::Profile, Driver};
use kernel::{
    dma_buf::DmaBuf,
    dma_fence::Fence,
    drm::device::RegisteredDeviceRef,
    prelude::*,
    sync::{aref::ARef, Arc, Mutex},
};

enum State {
    Empty,
    Configured { configuration: Arc<Configuration>, pool: Pool },
    Publishing,
    Ready { publication: Publication, pool: Pool, source: stream::Stream, output: output::Stream },
    Closed,
}

/// Advisory lifecycle metadata, separate from accepted KMS selection and GPU completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Empty,
    Configured,
    Publishing,
    Published,
    Withdrawn,
}

pub(crate) struct Description {
    pub(crate) phase: Phase,
    pub(crate) constraints_id: u64,
}

/// One worker generation at a time. A same-master reacquisition can configure a fresh one.
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

    /// Retire state from an earlier master interval without reviving any of its work.
    /// Outstanding claims keep their release channel and delay reuse of the endpoint.
    fn refresh_generation(&self) -> Result<Interval> {
        let interval = self.access.current_interval()?;
        let (retired, revocation, stale_id) = {
            let mut state = self.state.lock();
            match &*state {
                State::Configured { configuration, .. } if configuration.interval() != interval => (
                    Some(core::mem::replace(&mut *state, State::Empty)),
                    None,
                    None,
                ),
                State::Ready { publication, source, output, .. }
                    if publication.interval() != interval =>
                {
                    let id = publication.entry().id();
                    let revocation = Some(publication.revocation());
                    let retired = if source.idle() && output.idle() {
                        Some(core::mem::replace(&mut *state, State::Empty))
                    } else {
                        None
                    };
                    (retired, revocation, Some(id))
                }
                _ => (None, None, None),
            }
        };
        if let Some(revocation) = revocation {
            revocation.revoke();
        }
        drop(retired);

        // Revocation can make a concurrently publishing claim return its slot. Reap the
        // generation if it became idle, but never confuse it with a replacement identity.
        if let Some(id) = stale_id {
            let retired = {
                let mut state = self.state.lock();
                if matches!(&*state, State::Ready { publication, source, output, .. }
                    if publication.entry().id() == id && publication.interval() != interval
                        && source.idle() && output.idle())
                {
                    Some(core::mem::replace(&mut *state, State::Empty))
                } else {
                    None
                }
            };
            drop(retired);
        }
        Ok(interval)
    }

    /// Allocate an immutable declaration without changing native constraints availability.
    /// Failed preparation leaves an empty endpoint available for retry.
    pub(crate) fn declare(&self, profile: Profile, dimensions: [u32; 2]) -> Result {
        self.refresh_generation()?;
        let configuration = Arc::new(
            Configuration::new(self.access.clone(), profile, dimensions)?,
            GFP_KERNEL,
        )?;
        let pool = Pool::new()?;
        let mut state = self.state.lock();
        match &*state {
            State::Empty => (),
            State::Closed => return Err(EKEYREVOKED),
            _ => return Err(EALREADY),
        }
        let interval = configuration.interval();
        self.access.with_output_interval(interval, || {
            *state = State::Configured { configuration, pool };
            Ok(())
        })
    }

    pub(crate) fn register_image(
        &self,
        id: u64,
        dimensions: [u32; 2],
        buffers: &[ARef<DmaBuf>],
    ) -> Result {
        self.refresh_generation()?;
        let mut state = self.state.lock();
        match &mut *state {
            State::Configured { configuration, pool } => {
                if dimensions != configuration.dimensions() {
                    return Err(EINVAL);
                }
                pool.insert(id, || configuration.register_image(buffers))
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
                State::Configured { pool, .. } => pool.remove(id)?,
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

    pub(crate) fn changed(&self) -> &kernel::sync::poll::PollCondVar {
        &self.access.device().changed
    }

    /// Prepare and copy the reply outside endpoint exclusion, then serialize listing and owner
    /// install. No fallible operation follows successful listing. A concurrent close or master
    /// change after copyout fails publication; callers must discard result bytes on error.
    pub(crate) fn publish(
        &self,
        completion: Option<ARef<Fence>>,
        reply: impl FnOnce(u64) -> Result,
    ) -> Result {
        self.refresh_generation()?;
        let resources = {
            let mut state = self.state.lock();
            match &*state {
                State::Configured { .. } => (),
                State::Closed => return Err(EKEYREVOKED),
                State::Empty => return Err(ENODATA),
                State::Publishing => return Err(EBUSY),
                State::Ready { .. } => return Err(EALREADY),
            }
            core::mem::replace(&mut *state, State::Publishing)
        };
        let mut pending = Pending { endpoint: self, resources: Some(resources) };
        let Some(State::Configured { configuration, pool }) = &pending.resources else {
            return Err(EIO);
        };
        // This owner is declared before the lock, so errors revoke it after lock release.
        let publication = {
            let registered = self.device.registration_guard().ok_or(ENODEV)?;
            Publication::new(&registered, configuration, pool, completion.as_deref())?
        };
        publication.prepare_reply(reply)?;
        let registered = self.device.registration_guard().ok_or(ENODEV)?;
        let mut state = self.state.lock();
        if matches!(&*state, State::Closed) {
            return Err(EKEYREVOKED);
        }
        if !matches!(&*state, State::Publishing) {
            return Err(ECANCELED);
        }
        // Extract all fallible bookkeeping before making the publication selectable.
        let Some(State::Configured { configuration, pool }) = pending.resources.take() else {
            return Err(EIO);
        };
        let result = publication.publish(&registered);
        match result {
            Ok(()) => *state = State::Ready {
                publication,
                pool,
                source: stream::Stream::new(),
                output: output::Stream::new(),
            },
            Err(error) => {
                drop(state);
                pending.resources = Some(State::Configured { configuration, pool });
                return Err(error);
            }
        }
        drop(state);
        drop(configuration);
        Ok(())
    }

    /// Advisory identity only, not a promise that an entry is selected or still ready.
    pub(crate) fn constraints_id(&self) -> Result<u64> {
        Ok(self.describe()?.constraints_id)
    }

    pub(crate) fn describe(&self) -> Result<Description> {
        self.refresh_generation()?;
        let state = self.state.lock();
        let (phase, constraints_id) = match &*state {
            State::Closed => return Err(EKEYREVOKED),
            State::Empty => (Phase::Empty, 0),
            State::Configured { .. } => (Phase::Configured, 0),
            State::Publishing => (Phase::Publishing, 0),
            State::Ready { publication, .. } => (
                if publication.is_live() { Phase::Published } else { Phase::Withdrawn },
                publication.entry().id(),
            ),
        };
        Ok(Description { phase, constraints_id })
    }

    /// Stop new selection and admission without closing the outstanding release channel.
    /// Cleanup authority does not require current pixel permission. Repeated withdrawal
    /// waits for the same terminal callback; accepted state and submitted reads are retained.
    pub(crate) fn withdraw(&self) -> Result {
        let revocation = {
            let state = self.state.lock();
            match &*state {
                State::Ready { publication, .. } => publication.revocation(),
                State::Closed => return Err(EKEYREVOKED),
                State::Publishing => return Err(EBUSY),
                _ => return Err(ENODATA),
            }
        };
        revocation.revoke();
        self.access.device().changed.notify_all();
        Ok(())
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
