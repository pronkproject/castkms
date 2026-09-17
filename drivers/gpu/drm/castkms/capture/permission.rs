// SPDX-License-Identifier: GPL-2.0-only

//! Current access to one output, independent of grant transport and pixel storage.

pub(crate) use crate::image_access::Current;
use crate::{
    authority::Interval,
    display,
    display_control,
    Driver, //
};
use kernel::{
    drm::{
        auth::CurrentMasterGuard,
        kms::{
            connector::Connector,
            crtc::Crtc, //
        },
        Device, //
    },
    prelude::*, //
};

/// Retained recipient and exact display objects, not a continuing authorization token.
///
/// Drop outside native master, object-ID and modeset locks: the retained references may
/// perform final DRM cleanup. Device-owned state must not retain permissions indefinitely.
pub(crate) struct Permission {
    target: display_control::Target,
    interval: Option<Interval>,
}

impl Permission {
    /// Borrow the exact target's device for resource operations, not pixel authorization.
    pub(super) fn device(&self) -> &Device<Driver> {
        self.target.device()
    }

    pub(super) fn display(&self) -> &crate::device::Display {
        self.target.display()
    }

    /// Establish a kernel-issued target from stabilized top-level display control.
    ///
    /// File issuance must additionally verify the issuing file's master role and retain
    /// its revocation ownership. A retained master snapshot alone is not accepted here.
    pub(crate) fn new(
        guard: &CurrentMasterGuard<'_, Driver>,
        crtc: &Crtc<display::Crtc>,
        connector: &Connector<display::Connector>,
    ) -> Result<Self> {
        Ok(Self {
            target: display_control::Target::new(guard, crtc, connector)?,
            interval: None,
        })
    }

    /// Bind administrative issuance to one uninterrupted top-level owner interval.
    pub(crate) fn administrative(
        guard: &CurrentMasterGuard<'_, Driver>,
        crtc: &Crtc<display::Crtc>,
        connector: &Connector<display::Connector>,
        interval: Interval,
    ) -> Result<Self> {
        Ok(Self {
            target: display_control::Target::new(guard, crtc, connector)?,
            interval: Some(interval),
        })
    }

    fn check_interval(&self) -> Result {
        let Some(expected) = self.interval else {
            return Ok(());
        };
        match self.device().authority.interval() {
            Ok(current) if current == expected => Ok(()),
            Err(error) if error == ENODEV => Err(error),
            _ => Err(ESTALE),
        }
    }

    /// Check access and accepted ownership while excluding changes through the callback.
    ///
    /// Lock order is native master, object IDs, then accepted output. The callback may
    /// register or claim a native capture job, but must not read pixels, acquire modeset
    /// locks, wait for rendering, or release final DRM object references. Returned data
    /// is historical metadata, not permission to make a later unchecked claim.
    pub(crate) fn with_current<R>(&self, f: impl FnOnce(Current<'_>) -> Result<R>) -> Result<R> {
        self.check_interval()?;
        self.target.with_current(|control| {
            self.check_interval()?;
            f(Current::new(control)?)
        })
    }

    /// Stabilize pixel ownership without selecting a HOST image layout.
    /// The callback follows the same locking restrictions as `with_current`.
    pub(super) fn with_control<R>(
        &self,
        f: impl FnOnce(display_control::Current<'_>) -> Result<R>,
    ) -> Result<R> {
        self.check_interval()?;
        self.target.with_current(|current| {
            self.check_interval()?;
            current.check_scene_owner()?;
            f(current)
        })
    }

    /// Match a renderer's stabilized display scope without recursively taking DRM locks.
    pub(super) fn check_control(&self, current: &display_control::Current<'_>) -> Result {
        self.check_interval()?;
        current.check_target(&self.target)?;
        current.check_scene_owner()
    }
}
