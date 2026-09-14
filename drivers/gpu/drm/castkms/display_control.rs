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

    /// Inspect control only after the latest accepted CRTC state has installed its scene.
    ///
    /// Registration is followed by master, CRTC, object-ID and output locks, in that order.
    /// The callback runs once with those locks held, without waiting for commit-tail progress.
    /// It follows `with_current`'s restrictions and must not acquire another modeset lock.
    /// An accepted predecessor that has not installed its scene returns `EAGAIN` instead of
    /// permitting control changes against the preceding publication.
    pub(crate) fn with_installed<R>(
        &self,
        registered: &Device<Driver, Registered>,
        f: impl FnOnce(Current<'_>) -> Result<R>,
    ) -> Result<R> {
        let identity = self.master.lock_current_identity().ok_or(EACCES)?;
        registered.with_crtc_preparation_source(self.crtc.crtc(), |source| {
            let source = source.ok_or(EAGAIN)?;
            identity.with_objects(|guard| {
                self.with_guard(guard, |current| {
                    current.check_source(source)?;
                    f(current)
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
        let output = &self.device().output;
        output.with_accepted(|accepted| {
            let accepted = accepted.ok_or(ENODEV)?;
            let configuration = accepted.configuration.as_ref().ok_or(ENODEV)?;
            if configuration.connector_mask() & self.connector.mask() == 0 {
                return Err(EACCES);
            }
            f(Current {
                configuration,
                output: output.identity(),
                master: &self.master,
                scene: accepted.scene,
                source: accepted.source,
                _task: NotThreadSafe,
            })
        })
    }
}

/// Callback-local control of an enabled output, without access to scene storage.
pub(crate) struct Current<'a> {
    configuration: &'a Configuration,
    output: &'a Identity,
    master: &'a MasterRef<Driver>,
    scene: Option<&'a Scene>,
    source: &'a Source,
    _task: NotThreadSafe,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Current<'_> {
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
}
