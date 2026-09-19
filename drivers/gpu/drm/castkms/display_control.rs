// SPDX-License-Identifier: GPL-2.0-only

//! Current control of exact display objects, independent of capture and renderer policy.

use crate::{
    display,
    output::Identity,
    scene::{
        Configuration,
        Scene, //
    },
    Driver, //
};
use kernel::{
    dma_resv::Reservation,
    drm::{
        auth::{
            CurrentMasterGuard,
            MasterRef, //
        },
        device::Registered,
        kms::{
            connector::{
                Connector,
                RawConnector, //
            },
            crtc::{
                Crtc,
                CrtcRef, //
            }, //
            LockedState,
        },
        preparation::Source,
        Device, //
    },
    prelude::*,
    sync::aref::ARef,
    types::NotThreadSafe, //
};

/// Retained display objects and master identity, not continuing permission to use them.
///
/// Drop outside native master, object-ID and modeset locks: final DRM cleanup may run.
/// Device-owned state must not retain targets indefinitely.
pub(crate) struct Target {
    master: MasterRef<Driver>,
    crtc: CrtcRef<display::Crtc>,
    connector: ARef<Connector<display::Connector>>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Target {
    /// Retain a target while the top-level master's control is stabilized.
    ///
    /// The caller separately establishes its operation's authority and revocation owner.
    /// Neither a retained master snapshot nor an ordinary capture handle is accepted here.
    pub(crate) fn new(
        guard: &CurrentMasterGuard<'_, Driver>,
        crtc: &Crtc<display::Crtc>,
        connector: &Connector<display::Connector>,
    ) -> Result<Self> {
        if guard.master().is_lessee() || !guard.holds_object(crtc) || !guard.holds_object(connector)
        {
            return Err(EACCES);
        }
        Ok(Self {
            master: guard.master().clone(),
            crtc: crtc.to_owned_ref(),
            connector: connector.into(),
        })
    }

    /// Borrow the owning device for resource operations, without authorizing pixel access.
    pub(crate) fn device(&self) -> &Device<Driver> {
        self.crtc.drm_dev()
    }

    /// Borrow static provider topology without taking authority or modeset locks.
    /// This permits retirement cleanup outside locks; callers authorize publication separately.
    pub(crate) fn constraints_output<'a>(
        &'a self,
        registered: &'a Device<Driver, Registered>,
    ) -> Result<kernel::drm::kms::constraints::Output<'a, Driver>> {
        registered.constraints_output(self.crtc.crtc())
    }

    /// Stabilize ownership of one static output pair without requiring an active video mode.
    ///
    /// Metadata and audio authority may survive disabled video. No scene,
    /// source-storage or renderer permission is implied. The callback runs under native
    /// master and object-ID locks and must not acquire modeset or monitor-description locks.
    pub(crate) fn with_output_objects<R>(&self, f: impl FnOnce() -> Result<R>) -> Result<R> {
        let guard = self.master.lock_current().ok_or(EACCES)?;
        if !guard.holds_object(self.crtc.crtc())
            || !guard.holds_object(&*self.connector)
            || !kernel::sync::Arc::ptr_eq(
                &self.crtc.crtc().display.monitor,
                &self.connector.monitor,
            )
        {
            return Err(EACCES);
        }
        f()
    }

    /// Inspect control only after the latest accepted CRTC state has installed its scene.
    ///
    /// Registration is followed by master, modeset, object-ID and output locks, in that order.
    /// The callback runs once with those locks held, without waiting for commit-tail progress.
    /// It follows `with_current`'s restrictions and must not acquire another modeset lock.
    /// An accepted predecessor that has not installed its scene returns `EAGAIN` instead of
    /// permitting control changes against the preceding publication.
    pub(crate) fn with_installed<R>(
        &self,
        registered: &Device<Driver, Registered>,
        f: impl FnOnce(Current<'_>, &LockedState<'_, Driver>) -> Result<R>,
    ) -> Result<R> {
        let device: &Device<Driver> = registered;
        if !core::ptr::eq(self.device(), device) {
            return Err(EINVAL);
        }
        let identity = self.master.lock_current_identity().ok_or(EACCES)?;
        registered.with_modeset_locks(|locked| {
            let source = locked.preparation_source(self.crtc.crtc())?.ok_or(EAGAIN)?;
            identity.with_objects(|guard| {
                self.with_guard(guard, |current| {
                    current.check_source(source)?;
                    f(current, locked)
                })
            })
        })?
    }

    /// Inspect current control and configuration while excluding their replacement.
    ///
    /// Lock order is native master, object IDs, then accepted output. The callback must
    /// not read pixels, acquire modeset locks, wait for rendering, or release final DRM
    /// references. It may acquire inner admission locks in the caller's documented order.
    /// Retained observations are historical metadata, not later admission or activation.
    pub(crate) fn with_current<R>(&self, f: impl FnOnce(Current<'_>) -> Result<R>) -> Result<R> {
        let guard = self.master.lock_current().ok_or(EACCES)?;
        self.with_guard(&guard, f)
    }

    fn with_guard<R>(
        &self,
        guard: &CurrentMasterGuard<'_, Driver>,
        f: impl FnOnce(Current<'_>) -> Result<R>,
    ) -> Result<R> {
        if !guard.holds_object(self.crtc.crtc()) || !guard.holds_object(&*self.connector) {
            return Err(EACCES);
        }
        let output = &self.crtc.crtc().display.output;
        output.with_accepted(|accepted| {
            let accepted = accepted.ok_or(ENODEV)?;
            let configuration = accepted.configuration.as_ref().ok_or(ENODEV)?;
            if configuration.connector_mask() & self.connector.mask() == 0 {
                return Err(EACCES);
            }
            f(Current {
                guard,
                configuration,
                output: output.identity(),
                master: &self.master,
                scene: accepted.scene,
                source: accepted.source,
                _task: NotThreadSafe,
            })
        })
    }

    pub(crate) fn display(&self) -> &crate::device::Display {
        &self.crtc.crtc().display
    }
}

/// Callback-local control of an enabled output, without access to scene storage.
pub(crate) struct Current<'a> {
    guard: &'a CurrentMasterGuard<'a, Driver>,
    configuration: &'a Configuration,
    output: &'a Identity,
    master: &'a MasterRef<Driver>,
    scene: Option<&'a Scene>,
    source: &'a Source,
    _task: NotThreadSafe,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Current<'_> {
    /// Check another capability's exact target using the already held native locks.
    /// This establishes matching control, not that capability's revocation policy.
    pub(crate) fn check_target(&self, target: &Target) -> Result {
        if self.master != &target.master
            || self.output != target.display().output.identity()
            || !self.guard.holds_object(target.crtc.crtc())
            || !self.guard.holds_object(&*target.connector)
            || self.configuration.connector_mask() & target.connector.mask() == 0
        {
            return Err(EACCES);
        }
        Ok(())
    }

    /// Inspect current backing identity without returning scene storage or a source claim.
    pub(crate) fn uses_reservation(&self, reservation: &Reservation) -> Result<bool> {
        self.scene
            .map_or(Ok(false), |scene| scene.uses_reservation(reservation))
    }

    /// Compare generation identity, not framebuffer identity or mode equality.
    ///
    /// The caller must separately stabilize the native accepted source for a control change.
    /// A retained source checked without modeset exclusion is only a historical observation.
    pub(crate) fn check_source(&self, source: &Source) -> Result {
        if core::ptr::eq(self.source, source) {
            Ok(())
        } else {
            Err(EAGAIN)
        }
    }

    pub(crate) fn configuration(&self) -> &Configuration {
        self.configuration
    }

    pub(crate) fn output_identity(&self) -> &Identity {
        self.output
    }

    pub(crate) fn master(&self) -> &MasterRef<Driver> {
        self.master
    }

    /// Check attribution separately from control; success grants no pixel or source claim.
    pub(crate) fn check_scene_owner(&self) -> Result {
        let scene = self.scene.ok_or(EAGAIN)?;
        if scene.owner() != Some(self.master) {
            return Err(EACCES);
        }
        Ok(())
    }

    /// HOST image admission follows the accepted backend, not framebuffer CPU readability.
    pub(crate) fn check_host_image(&self) -> Result {
        if !self.scene.ok_or(EAGAIN)?.host_binding() {
            return Err(EOPNOTSUPP);
        }
        Ok(())
    }

    /// Compare exact accepted backend identity while publication is excluded.
    /// Matching metadata alone grants neither pixels nor readiness for a new read.
    pub(crate) fn check_constraints(
        &self,
        entry: Option<&kernel::drm::constraints::OpaqueEntry>,
    ) -> Result {
        let scene = self.scene.ok_or(EAGAIN)?;
        if scene.constraints().map(core::ptr::from_ref) != entry.map(core::ptr::from_ref) {
            return Err(ESTALE);
        }
        Ok(())
    }

    pub(crate) fn renderer_worker(
        &self,
    ) -> Result<kernel::sync::Arc<crate::renderer::ready::Worker>> {
        self.scene.ok_or(EAGAIN)?.renderer_worker()
    }

    /// Claim and retain the current scene while its authority and generation are stable.
    ///
    /// The claim, not the cloned scene, prevents source retirement. The caller must not
    /// publish either owner until every enclosing authorization callback has succeeded.
    pub(crate) fn claim_changed_scene(
        &self,
        previous_content_serial: Option<u64>,
    ) -> Result<(Scene, kernel::drm::preparation::ReadClaim)> {
        self.changed_content(previous_content_serial)?;
        let scene = self.scene.ok_or(EAGAIN)?;
        let claim = self.source.claim()?;
        Ok((scene.clone(), claim))
    }

    /// Observe a changed content identity without reserving storage or a source read.
    /// Admission must repeat this check after any unlocked preparation interval.
    pub(crate) fn changed_content(
        &self,
        previous_content_serial: Option<u64>,
    ) -> Result<crate::scene::ContentSerial> {
        self.check_scene_owner()?;
        let scene = self.scene.ok_or(EAGAIN)?;
        let content_serial = scene.render_content().ok_or(ENODATA)?;
        if previous_content_serial == Some(content_serial.get()) {
            return Err(ENODATA);
        }
        Ok(content_serial)
    }

    /// An active blank output must not deliver pixels from an earlier revision.
    pub(crate) fn check_blank_render_content(
        &self,
        content: Option<crate::scene::ContentSerial>,
    ) -> Result {
        let scene = self.scene.ok_or(EAGAIN)?;
        if scene.layers().next().is_none() && scene.render_content() != content {
            return Err(ESTALE);
        }
        Ok(())
    }
}
