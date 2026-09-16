// SPDX-License-Identifier: GPL-2.0-only

//! Startup reservation and active-renderer ownership, without pixel authority.

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
        poll::PollCondVar,
        Arc,
        Mutex, //
    }, //
};

enum State {
    Idle,
    Reserved(Arc<()>),
    Active(Arc<()>),
    Replacing { active: Arc<()>, candidate: Arc<()> },
    Lost,
    Closed,
}

/// One output's candidates share a budget across cancellation and replacement.
///
/// This object retains neither the DRM device nor a scene. The registration owner
/// closes it before releasing display state; outstanding copies keep their own storage.
#[pin_data]
pub(crate) struct Startup {
    output: Identity,
    changed: Arc<PollCondVar>,
    budget: Budget,
    #[pin]
    state: Mutex<State>,
}

impl Startup {
    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn reserve_for_test(
        &self,
        device: &Device<Driver>,
        bytes: usize,
    ) -> Result<impl Sized> {
        self.budget.reserve_for_test(device, bytes)
    }

    /// Reserve a candidate without changing the active renderer or claiming sources.
    pub(crate) fn begin(self: &Arc<Self>) -> Result<Candidate> {
        let identity = Arc::new((), GFP_KERNEL)?;
        {
            let mut state = self.state.lock();
            match &*state {
                State::Idle => *state = State::Reserved(identity.clone()),
                State::Active(active) => {
                    *state = State::Replacing {
                        active: active.clone(),
                        candidate: identity.clone(),
                    }
                }
                State::Reserved(_) | State::Replacing { .. } | State::Lost => return Err(EBUSY),
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
            State::Replacing { candidate, .. } if Arc::ptr_eq(candidate, identity) => f(),
            State::Closed => Err(ENODEV),
            _ => Err(ECANCELED),
        }
    }

    fn activate<R>(
        self: &Arc<Self>,
        identity: &Arc<()>,
        publish: impl FnOnce() -> Result<R>,
    ) -> Result<(Active, R)> {
        let mut state = self.state.lock();
        match &*state {
            State::Reserved(current) if Arc::ptr_eq(current, identity) => (),
            State::Replacing { candidate, .. } if Arc::ptr_eq(candidate, identity) => (),
            State::Closed => return Err(ENODEV),
            _ => return Err(ECANCELED),
        }
        let result = publish()?;
        *state = State::Active(identity.clone());
        self.changed.notify_all();
        Ok((
            Active {
                startup: self.clone(),
                identity: identity.clone(),
            },
            result,
        ))
    }

    fn cancel(&self, identity: &Arc<()>) {
        let retired = {
            let mut state = self.state.lock();
            match &*state {
                State::Reserved(current) if Arc::ptr_eq(current, identity) => {
                    Some(core::mem::replace(&mut *state, State::Idle))
                }
                State::Replacing { active, candidate } if Arc::ptr_eq(candidate, identity) => {
                    let restored = State::Active(active.clone());
                    Some(core::mem::replace(&mut *state, restored))
                }
                _ => None,
            }
        };
        drop(retired);
        self.changed.notify_all();
    }

    fn handback<R>(&self, identity: &Arc<()>, publish: impl FnOnce() -> Result<R>) -> Result<R> {
        let mut state = self.state.lock();
        match &*state {
            State::Reserved(current) if Arc::ptr_eq(current, identity) => (),
            State::Replacing { candidate, .. } if Arc::ptr_eq(candidate, identity) => (),
            State::Closed => return Err(ENODEV),
            _ => return Err(ECANCELED),
        }
        let result = publish()?;
        *state = State::Idle;
        self.changed.notify_all();
        Ok(result)
    }

    /// Invalidate startup state belonging to a replaced display-control interval.
    pub(crate) fn invalidate_current(&self) {
        self.invalidate(false);
    }

    /// Negotiated workers follow profile-validated mode changes without losing ownership.
    pub(crate) fn configuration_changed(&self) {
        self.invalidate(true);
    }

    fn invalidate(&self, configuration_only: bool) {
        let retired = {
            let mut state = self.state.lock();
            match &*state {
                State::Reserved(_) => Some(core::mem::replace(&mut *state, State::Idle)),
                State::Active(_) if configuration_only => None,
                State::Replacing { active, .. } if configuration_only => {
                    let restored = State::Active(active.clone());
                    Some(core::mem::replace(&mut *state, restored))
                }
                State::Replacing { .. } => Some(core::mem::replace(&mut *state, State::Lost)),
                State::Active(..) => Some(core::mem::replace(&mut *state, State::Lost)),
                _ => None,
            }
        };
        drop(retired);
        self.changed.notify_all();
    }

    fn lose(&self, identity: &Arc<()>) {
        let retired = {
            let mut state = self.state.lock();
            match &*state {
                State::Active(current) if Arc::ptr_eq(current, identity) => {
                    Some(core::mem::replace(&mut *state, State::Lost))
                }
                State::Replacing { active, .. } if Arc::ptr_eq(active, identity) => {
                    Some(core::mem::replace(&mut *state, State::Lost))
                }
                _ => None,
            }
        };
        drop(retired);
        self.changed.notify_all();
    }

    fn close(&self) {
        let retired = core::mem::replace(&mut *self.state.lock(), State::Closed);
        drop(retired);
        self.changed.notify_all();
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

    /// Transfer this reservation into active-renderer ownership after publication.
    ///
    /// The callback and state transition run under the startup lock. Callback failure
    /// leaves the candidate reserved. The returned owner marks unexpected loss when dropped;
    /// it never reopens candidate admission.
    pub(crate) fn activate<R>(&self, publish: impl FnOnce() -> Result<R>) -> Result<(Active, R)> {
        self.startup.activate(&self.identity, publish)
    }

    /// Stop old renderer admission only after a gated HOST publication succeeds.
    pub(crate) fn handback<R>(&self, publish: impl FnOnce() -> Result<R>) -> Result<R> {
        self.startup.handback(&self.identity, publish)
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

/// Device-wide active-renderer identity transferred from one startup candidate.
#[must_use = "dropping active renderer ownership records terminal loss"]
pub(crate) struct Active {
    startup: Arc<Startup>,
    identity: Arc<()>,
}

impl Active {
    /// Observe this incarnation without extending its unique active-owner lifetime.
    pub(crate) fn observation(&self) -> Observation {
        Observation {
            startup: self.startup.clone(),
            identity: self.identity.clone(),
        }
    }

    /// Check that this renderer remains the active device-wide incarnation.
    pub(crate) fn check(&self) -> Result {
        match &*self.startup.state.lock() {
            State::Active(current) if Arc::ptr_eq(current, &self.identity) => Ok(()),
            State::Replacing { active, .. } if Arc::ptr_eq(active, &self.identity) => Ok(()),
            State::Closed => Err(ENODEV),
            _ => Err(EIO),
        }
    }

    /// Run an operation while this exact active incarnation cannot be invalidated.
    ///
    /// The callback holds the startup lock and must not reenter startup control, wait for
    /// work, acquire outer locks or release final DRM references.
    pub(crate) fn with_current<R>(&self, f: impl FnOnce() -> Result<R>) -> Result<R> {
        match &*self.startup.state.lock() {
            State::Active(current) if Arc::ptr_eq(current, &self.identity) => f(),
            State::Replacing { active, .. } if Arc::ptr_eq(active, &self.identity) => f(),
            State::Closed => Err(ENODEV),
            _ => Err(EIO),
        }
    }

    /// Require ownership transferred from the named candidate before running an operation.
    pub(crate) fn with_candidate<R>(
        &self,
        candidate: &Candidate,
        f: impl FnOnce() -> Result<R>,
    ) -> Result<R> {
        if !Arc::ptr_eq(&self.startup, &candidate.startup)
            || !Arc::ptr_eq(&self.identity, &candidate.identity)
        {
            return Err(EINVAL);
        }
        self.with_current(f)
    }
}

/// Retained identity only: dropping the active owner still invalidates every observation.
#[derive(Clone)]
pub(crate) struct Observation {
    startup: Arc<Startup>,
    identity: Arc<()>,
}

impl Observation {
    /// Apply the active incarnation's checks without keeping its owner alive.
    pub(crate) fn with_candidate<R>(
        &self,
        candidate: &Candidate,
        f: impl FnOnce() -> Result<R>,
    ) -> Result<R> {
        if !Arc::ptr_eq(&self.startup, &candidate.startup)
            || !Arc::ptr_eq(&self.identity, &candidate.identity)
        {
            return Err(EINVAL);
        }
        match &*self.startup.state.lock() {
            State::Active(current) if Arc::ptr_eq(current, &self.identity) => f(),
            State::Replacing { active, .. } if Arc::ptr_eq(active, &self.identity) => f(),
            State::Closed => Err(ENODEV),
            _ => Err(EIO),
        }
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        self.startup.lose(&self.identity);
    }
}

/// Unique shutdown owner; retained candidates cannot reopen a closed output.
pub(crate) struct Owner(Arc<Startup>);

impl Owner {
    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn new(output: &Identity) -> Result<Self> {
        Self::new_notified(output, Arc::pin_init(kernel::new_poll_condvar!(), GFP_KERNEL)?)
    }

    pub(crate) fn new_notified(output: &Identity, changed: Arc<PollCondVar>) -> Result<Self> {
        Ok(Self(Arc::pin_init(
            try_pin_init!(Startup {
                output: output.clone(),
                changed,
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
