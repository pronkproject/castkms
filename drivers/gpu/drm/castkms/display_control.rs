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

    /// Inspect current control and configuration while excluding their replacement.
    ///
    /// Lock order is native master, object IDs, then accepted output. The callback must
    /// not read pixels, acquire modeset locks, wait for rendering, or release final DRM
    /// references. It may acquire inner admission locks in the caller's documented order.
    /// Retained observations are historical metadata, not later admission or activation.
    pub(crate) fn with_current<R>(&self, f: impl FnOnce(Current<'_>) -> Result<R>) -> Result<R> {
        let guard = self.master.lock_current().ok_or(EACCES)?;
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
    _task: NotThreadSafe,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Current<'_> {
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
