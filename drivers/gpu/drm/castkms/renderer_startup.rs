// SPDX-License-Identifier: GPL-2.0-only

//! One candidate's private startup resources, without execution or pixel authority.

use crate::{
    host_compositor::compose::Completed,
    host_snapshot::{
        Budget,
        Snapshot, //
    },
    output::Identity,
    Driver, //
};
use kernel::{
    drm::Device,
    prelude::*,
    sync::{
        Arc,
        Mutex, //
    }, //
};

enum State {
    Idle,
    Reserved(Arc<()>),
    Closed,
}

/// One output's candidates share a budget across cancellation and replacement.
///
/// This object retains neither the DRM device nor a scene. The registration owner
/// closes it before releasing display state; outstanding copies keep their own storage.
#[pin_data]
pub(crate) struct Startup {
    output: Identity,
    budget: Budget,
    #[pin]
    state: Mutex<State>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Startup {
    /// Reserve a candidate without changing the active renderer or claiming sources.
    pub(crate) fn begin(self: &Arc<Self>) -> Result<Candidate> {
        let identity = Arc::new((), GFP_KERNEL)?;
        {
            let mut state = self.state.lock();
            match &*state {
                State::Idle => *state = State::Reserved(identity.clone()),
                State::Reserved(_) => return Err(EBUSY),
                State::Closed => return Err(ENODEV),
            }
        }
        Ok(Candidate {
            startup: self.clone(),
            identity,
        })
    }

    fn check(&self, identity: &Arc<()>) -> Result {
        self.with_current(identity, || Ok(()))
    }

    fn with_current<R>(&self, identity: &Arc<()>, f: impl FnOnce() -> Result<R>) -> Result<R> {
        match &*self.state.lock() {
            State::Reserved(current) if Arc::ptr_eq(current, identity) => f(),
            State::Closed => Err(ENODEV),
            _ => Err(ECANCELED),
        }
    }

    fn cancel(&self, identity: &Arc<()>) {
        let retired = {
            let mut state = self.state.lock();
            match &*state {
                State::Reserved(current) if Arc::ptr_eq(current, identity) => {
                    Some(core::mem::replace(&mut *state, State::Idle))
                }
                _ => None,
            }
        };
        drop(retired);
    }

    /// Cancel the old control interval's candidate without affecting a later replacement.
    pub(crate) fn cancel_current(&self) {
        let retired = {
            let mut state = self.state.lock();
            if matches!(*state, State::Reserved(_)) {
                Some(core::mem::replace(&mut *state, State::Idle))
            } else {
                None
            }
        };
        drop(retired);
    }

    fn close(&self) {
        let retired = core::mem::replace(&mut *self.state.lock(), State::Closed);
        drop(retired);
    }
}

/// Unique reservation; dropping or canceling it cannot remove a later candidate.
#[must_use = "dropping the candidate releases its startup reservation"]
pub(crate) struct Candidate {
    startup: Arc<Startup>,
    identity: Arc<()>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Candidate {
    /// Observe whether the reservation is current, without authorizing an operation.
    pub(crate) fn check(&self) -> Result {
        self.startup.check(&self.identity)
    }

    /// Run a bounded control operation while excluding cancellation and replacement.
    ///
    /// The callback holds the startup lock after any caller-owned display and permission
    /// locks. It must not reenter startup operations, read pixels, wait for work, acquire
    /// outer locks or release final DRM references. Success does not itself activate a
    /// renderer or transfer ownership of the reservation.
    pub(crate) fn with_current<R>(&self, f: impl FnOnce() -> Result<R>) -> Result<R> {
        self.startup.with_current(&self.identity, f)
    }

    pub(crate) fn cancel(&self) {
        self.startup.cancel(&self.identity);
    }

    /// Make an optional private copy, not an authorized export or an activation token.
    ///
    /// The current candidate is checked on both sides of copying. The image's authority
    /// and configuration still need current validation before delivery. Allocation and
    /// copying run outside the startup lock and never claim a compositor source.
    pub(crate) fn snapshot(&self, device: &Device<Driver>, image: &Completed) -> Result<Snapshot> {
        self.snapshot_then(device, image, || {})
    }

    fn snapshot_then(
        &self,
        device: &Device<Driver>,
        image: &Completed,
        after_copy: impl FnOnce(),
    ) -> Result<Snapshot> {
        self.startup.check(&self.identity)?;
        if image.output_identity() != &self.startup.output {
            return Err(EINVAL);
        }
        let snapshot = Snapshot::new(device, &self.startup.budget, image)?;
        after_copy();
        self.startup.check(&self.identity)?;
        Ok(snapshot)
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn snapshot_then_for_test(
        &self,
        device: &Device<Driver>,
        image: &Completed,
        after_copy: impl FnOnce(),
    ) -> Result<Snapshot> {
        self.snapshot_then(device, image, after_copy)
    }
}

impl Drop for Candidate {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Unique shutdown owner; retained candidates cannot reopen a closed output.
pub(crate) struct Owner(Arc<Startup>);

impl Owner {
    pub(crate) fn new(output: &Identity) -> Result<Self> {
        Ok(Self(Arc::pin_init(
            try_pin_init!(Startup {
                output: output.clone(),
                budget: Budget::new()?,
                state <- kernel::new_mutex!(State::Idle),
            }),
            GFP_KERNEL,
        )?))
    }

    pub(crate) fn startup(&self) -> Arc<Startup> {
        self.0.clone()
    }

    pub(crate) fn close(&self) {
        self.0.close();
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
