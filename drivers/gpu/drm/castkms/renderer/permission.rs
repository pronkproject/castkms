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
        device::Registered,
        kms::{
            connector::Connector,
            crtc::Crtc,
            LockedState, //
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
    transition_owner: Arc<()>,
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

impl Owner {
    /// Allocate the owner outside native master, object-ID and modeset locks.
    pub(crate) fn new(permission: Permission) -> Result<Self> {
        let transition_owner = Arc::new((), GFP_KERNEL)?;
        let policy = Arc::pin_init(
            pin_init!(Policy {
                permission,
                transition_owner,
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
        let mut revoked = self.access.policy.revoked.lock();
        *revoked = true;
        self.access
            .device()
            .validation
            .revoke_owner(&self.access.policy.transition_owner);
        drop(revoked);
        self.access.device().changed.notify_all();
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
    /// Identity only, with no authority or retained DRM resources.
    pub(crate) fn transition_owner(&self) -> Arc<()> {
        self.policy.transition_owner.clone()
    }

    /// Borrow the allocation device without authorizing access to display pixels.
    pub(crate) fn device(&self) -> &Device<Driver> {
        self.policy.permission.target.device()
    }

    pub(crate) fn display(&self) -> &crate::device::Display {
        self.policy.permission.target.display()
    }

    /// Authorize a control operation without requiring ownership of the displayed image.
    ///
    /// Native master, object-ID and accepted-output locks precede the revocation lock.
    /// The callback follows Target::with_current's restrictions and must not revoke or
    /// release its owner. No returned observation authorizes a later unchecked operation.
    pub(crate) fn with_current<R>(&self, f: impl FnOnce(Current<'_>) -> Result<R>) -> Result<R> {
        self.policy
            .permission
            .target
            .with_current(|current| self.authorize(current, f))
    }

    /// Authorize output-scoped metadata without requiring enabled video or a scene.
    pub(crate) fn with_output<R>(&self, f: impl FnOnce() -> Result<R>) -> Result<R> {
        self.policy.permission.target.with_output_objects(|| {
            if self.device().authority.interval()? != self.policy.permission.interval {
                return Err(ESTALE);
            }
            let revoked = self.policy.revoked.lock();
            if *revoked {
                return Err(EKEYREVOKED);
            }
            f()
        })
    }

    /// Check revocation inside the installed-generation control interval.
    ///
    /// Master, modeset, object-ID and output locks precede the revocation lock. The callback
    /// follows `Target::with_installed`'s restrictions and must not revoke or drop its owner.
    pub(crate) fn with_installed<R>(
        &self,
        registered: &Device<Driver, Registered>,
        f: impl FnOnce(Current<'_>, &LockedState<'_, Driver>) -> Result<R>,
    ) -> Result<R> {
        self.policy
            .permission
            .target
            .with_installed(registered, |current, locked| {
                self.authorize(current, |current| f(current, locked))
            })
    }

    pub(crate) fn with_installed_transition<R>(
        &self,
        registered: &Device<Driver, Registered>,
        f: impl FnOnce(
            crate::display_control::TransitionCurrent<'_>,
            &LockedState<'_, Driver>,
        ) -> Result<R>,
    ) -> Result<R> {
        self.policy
            .permission
            .target
            .with_installed_transition(registered, |current, locked| {
                let permission = &self.policy.permission;
                if permission.target.device().authority.interval()? != permission.interval {
                    return Err(ESTALE);
                }
                let revoked = self.policy.revoked.lock();
                if *revoked {
                    return Err(EKEYREVOKED);
                }
                f(current, locked)
            })
    }

    fn authorize<R>(
        &self,
        current: Current<'_>,
        f: impl FnOnce(Current<'_>) -> Result<R>,
    ) -> Result<R> {
        let permission = &self.policy.permission;
        if permission.target.device().authority.interval()? != permission.interval {
            return Err(ESTALE);
        }
        let revoked = self.policy.revoked.lock();
        if *revoked {
            return Err(EKEYREVOKED);
        }
        f(current)
    }
}
