// SPDX-License-Identifier: GPL-2.0-only

//! Current access to one output, independent of grant transport and pixel storage.

pub(crate) use crate::image_access::Current;
use crate::{
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
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Permission {
    /// Borrow the exact target's device for resource operations, not pixel authorization.
    pub(super) fn device(&self) -> &Device<Driver> {
        self.target.device()
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
        })
    }

    /// Check access and accepted ownership while excluding changes through the callback.
    ///
    /// Lock order is native master, object IDs, then accepted output. The callback may
    /// register or claim a native capture job, but must not read pixels, acquire modeset
    /// locks, wait for rendering, or release final DRM object references. Returned data
    /// is historical metadata, not permission to make a later unchecked claim.
    pub(crate) fn with_current<R>(&self, f: impl FnOnce(Current<'_>) -> Result<R>) -> Result<R> {
        self.target
            .with_current(|control| f(Current::new(control)?))
    }
}
