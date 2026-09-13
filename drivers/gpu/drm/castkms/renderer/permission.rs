// SPDX-License-Identifier: GPL-2.0-only

//! Revocable renderer control, without source access or execution activation.

use crate::{
    authority::Interval,
    display,
    display_control::{
        Current,
        Target, //
    },
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
    prelude::*,
    sync::{
        Arc,
        Mutex, //
    }, //
};

/// Exact display control for one uninterrupted master interval.
///
/// Issuance needs a native control guard, not a capture capability. Constructing the
/// permission alone grants neither raw pixel access nor the right to activate execution.
/// Retained DRM objects must be released outside native master and modeset locks.
pub(crate) struct Permission {
    target: Target,
    interval: Interval,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Permission {
    pub(crate) fn new(
        guard: &CurrentMasterGuard<'_, Driver>,
        crtc: &Crtc<display::Crtc>,
        connector: &Connector<display::Connector>,
    ) -> Result<Self> {
        let target = Target::new(guard, crtc, connector)?;
        let interval = target.device().authority.interval()?;
        Ok(Self { target, interval })
    }
}

#[pin_data]
struct Policy {
    permission: Permission,
    #[pin]
    revoked: Mutex<bool>,
}

/// Unique issuer lifetime, distinct from retained access handles.
///
/// Revocation waits for in-progress authorization callbacks, not GPU work. Callback
/// results retain their own resources; separate operation owners arrange cleanup.
#[must_use = "dropping the owner revokes renderer access"]
pub(crate) struct Owner {
    access: Access,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Owner {
    /// Allocate the owner outside native master, object-ID and modeset locks.
    pub(crate) fn new(permission: Permission) -> Result<Self> {
        let policy = Arc::pin_init(
            pin_init!(Policy {
                permission,
                revoked <- kernel::new_mutex!(false),
            }),
            GFP_KERNEL,
        )?;
        Ok(Self {
            access: Access { policy },
        })
    }

    pub(crate) fn access(&self) -> Access {
        self.access.clone()
    }

    /// Permanently stop admission; call outside callbacks and native DRM locks.
    pub(crate) fn revoke(&self) {
        *self.access.policy.revoked.lock() = true;
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.revoke();
    }
}

/// Delegated renderer control with no ability to renew or transfer its issuer lifetime.
#[derive(Clone)]
pub(crate) struct Access {
    policy: Arc<Policy>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Access {
    /// Borrow the allocation device without authorizing access to display pixels.
    pub(crate) fn device(&self) -> &Device<Driver> {
        self.policy.permission.target.device()
    }

    /// Authorize a control operation without requiring ownership of the displayed image.
    ///
    /// Native master, object-ID and accepted-output locks precede the revocation lock.
    /// The callback follows Target::with_current's restrictions and must not revoke or
    /// release its owner. No returned observation authorizes a later unchecked operation.
    pub(crate) fn with_current<R>(&self, f: impl FnOnce(Current<'_>) -> Result<R>) -> Result<R> {
        let permission = &self.policy.permission;
        permission.target.with_current(|current| {
            if permission.target.device().authority.interval()? != permission.interval {
                return Err(ESTALE);
            }
            let revoked = self.policy.revoked.lock();
            if *revoked {
                return Err(EKEYREVOKED);
            }
            f(current)
        })
    }
}
