// SPDX-License-Identifier: GPL-2.0-only

//! Authorized private startup for one accepted display configuration.

use super::permission::Access;
use crate::{
    display_control,
    execution::Description,
    host_compositor::compose::Completed,
    host_snapshot::Snapshot,
    image_access,
    renderer_startup,
    scene::Configuration,
    Driver, //
};
use kernel::{
    drm::{
        device::Registered,
        kms::LockedState,
        Device, //
    },
    prelude::*, //
};

/// One renderer's private reservation, without an activation or live-source claim.
///
/// Configuration equality denotes the same mode and route interval, not equal dimensions.
/// Content-only updates do not invalidate startup. The access handle retains its issuer's
/// revocation state, not its issuer lifetime. Drop outside native DRM and admission locks.
#[must_use = "dropping the candidate releases its private startup reservation"]
pub(crate) struct Candidate {
    resources: renderer_startup::Candidate,
    access: Access,
    configuration: Configuration,
    execution: Description,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Candidate {
    /// Reserve outside policy locks, checking the configuration on both sides.
    pub(crate) fn begin(access: Access) -> Result<Self> {
        Self::begin_then(access, || Ok(()))
    }

    fn begin_then(access: Access, after_reserve: impl FnOnce() -> Result) -> Result<Self> {
        let configuration = access.with_current(|current| Ok(current.configuration().clone()))?;
        let execution = access.device().execution.describe();
        let resources = access.device().startup.begin()?;
        let candidate = Self {
            resources,
            access,
            configuration,
            execution,
        };
        after_reserve()?;
        candidate.validate()?;
        Ok(candidate)
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn begin_then_for_test(
        access: Access,
        after_reserve: impl FnOnce() -> Result,
    ) -> Result<Self> {
        Self::begin_then(access, after_reserve)
    }

    /// Historical configuration metadata, not permission to activate or read pixels.
    pub(crate) fn configuration(&self) -> &Configuration {
        &self.configuration
    }

    /// Recheck authority and private reservation without changing active execution.
    ///
    /// Success is an observation only. A later operation must perform its own validation
    /// and admission under the locks governing that operation.
    pub(crate) fn validate(&self) -> Result {
        self.with_current_control(|_| Ok(()))
    }

    /// Observe display metadata after checking the candidate under stable control.
    /// No lock survives the callback, and no later operation is reserved.
    fn with_current_control<R>(
        &self,
        f: impl FnOnce(display_control::Current<'_>) -> Result<R>,
    ) -> Result<R> {
        self.access
            .with_current(|current| self.with_reservation(current, f))
    }

    /// Stabilize installed display state, permission and this reservation for handoff.
    ///
    /// The callback holds master, modeset, object-ID, output, revocation and startup locks in
    /// that order. It must not wait, read pixels, acquire more modeset locks, cancel this
    /// candidate or release final resources. Only the callback may publish a control change;
    /// a returned observation does not reserve a later operation.
    pub(crate) fn with_activation_control<R>(
        &self,
        registered: &Device<Driver, Registered>,
        f: impl FnOnce(display_control::Current<'_>, &LockedState<'_, Driver>) -> Result<R>,
    ) -> Result<R> {
        self.access.with_installed(registered, |current, locked| {
            self.with_reservation(current, |current| f(current, locked))
        })
    }

    fn with_reservation<R>(
        &self,
        current: display_control::Current<'_>,
        f: impl FnOnce(display_control::Current<'_>) -> Result<R>,
    ) -> Result<R> {
        if current.configuration() != &self.configuration
            || self.access.device().execution.describe() != self.execution
        {
            return Err(ESTALE);
        }
        self.resources.with_current(|| f(current))
    }

    /// Copy an optional image into independent private storage after checking ownership.
    ///
    /// Current authority, display configuration and reservation are checked on both sides.
    /// Copying holds no policy lock and claims no compositor source. A returned snapshot
    /// retains its historical origin; it is neither an export nor permission to activate.
    /// Descriptor delivery must separately authorize its recipient at installation.
    pub(crate) fn snapshot(&self, image: &Completed) -> Result<Snapshot> {
        self.snapshot_then(image, || Ok(()))
    }

    fn snapshot_then(
        &self,
        image: &Completed,
        after_copy: impl FnOnce() -> Result,
    ) -> Result<Snapshot> {
        self.with_current_control(|control| {
            image_access::Current::new(control)?.check_image(image)
        })?;
        let snapshot = self.resources.snapshot(self.access.device(), image)?;
        after_copy()?;
        self.with_current_control(|control| {
            image_access::Current::new(control)?.check_snapshot(&snapshot)
        })?;
        Ok(snapshot)
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn snapshot_then_for_test(
        &self,
        image: &Completed,
        after_copy: impl FnOnce() -> Result,
    ) -> Result<Snapshot> {
        self.snapshot_then(image, after_copy)
    }

    /// Cancel only this reservation; retained objects cannot cancel its replacement.
    pub(crate) fn cancel(&self) {
        self.resources.cancel();
    }
}
