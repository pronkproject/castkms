// SPDX-License-Identifier: GPL-2.0-only

//! Revocable renderer authority, without source access or execution activation.

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
        capture::{Authority, Creator, Registration},
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
        aref::ARef,
        Arc,
        Mutex, //
    }, //
};

/// Exact display control bound to one native master identity.
///
/// Issuance needs a native control guard, not a capture capability. Constructing the
/// permission alone grants neither raw pixel access nor the right to activate execution.
/// Retained DRM objects must be released outside native master and modeset locks.
pub(crate) struct Permission {
    target: Target,
}

impl Permission {
    pub(crate) fn new(
        guard: &CurrentMasterGuard<'_, Driver>,
        crtc: &Crtc<display::Crtc>,
        connector: &Connector<display::Connector>,
    ) -> Result<Self> {
        Ok(Self {
            target: Target::new(guard, crtc, connector)?,
        })
    }
}

#[pin_data]
struct Policy {
    permission: Permission,
    workers: Arc<crate::authority::grants::Registry>,
    #[pin]
    revoked: Mutex<bool>,
}

// SAFETY: Native authority ownership retains the callback module and policy allocation.
#[vtable]
unsafe impl kernel::drm::capture::Policy for Policy {
    fn revoke(&self) {
        let mut revoked = self.revoked.lock();
        *revoked = true;
        drop(revoked);
        self.workers.close();
        self.permission.target.device().changed.notify_all();
    }
}

/// Unique issuer lifetime, distinct from retained access handles.
///
/// Revocation waits for in-progress authorization callbacks, not GPU work. Callback
/// results retain their own resources; separate operation owners arrange cleanup.
#[must_use = "dropping the owner revokes renderer access"]
pub(crate) struct Owner {
    access: Access,
    authority: ARef<Authority<Policy>>,
    creator: Option<Registration>,
}

impl Owner {
    /// Allocate the owner outside native master, object-ID and modeset locks.
    pub(crate) fn new(permission: Permission) -> Result<Self> {
        let workers = crate::authority::grants::Registry::new()?;
        let policy = Arc::pin_init(
            pin_init!(Policy {
                permission,
                workers,
                revoked <- kernel::new_mutex!(false),
            }),
            GFP_KERNEL,
        )?;
        let authority = Authority::new(policy.clone())?;
        Ok(Self {
            access: Access { policy },
            authority,
            creator: None,
        })
    }

    /// Attach issuer cleanup while the administrative file and bound master are stabilized.
    pub(crate) fn track_creator(&mut self, creator: &Creator) -> Result {
        if self.creator.is_some() {
            return Err(EEXIST);
        }
        self.creator = Some(creator.register(&self.authority)?);
        Ok(())
    }

    pub(crate) fn access(&self) -> Access {
        self.access.clone()
    }

    /// Permanently stop admission; call outside callbacks and native DRM locks.
    pub(crate) fn revoke(&self) {
        self.authority.revoke();
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.revoke();
    }
}

/// Delegated renderer authority with no ability to renew or transfer its issuer lifetime.
#[derive(Clone)]
pub(crate) struct Access {
    policy: Arc<Policy>,
}

pub(crate) struct WorkerRegistration {
    _permission: crate::authority::grants::Registration,
    _device: crate::authority::grants::Registration,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Access {
    /// Terminal issuer revocation, without requiring the bound master to be current.
    pub(crate) fn is_revoked(&self) -> bool {
        *self.policy.revoked.lock()
    }

    /// Observe the current work generation while the bound master is current.
    pub(crate) fn current_interval(&self) -> Result<Interval> {
        self.policy
            .permission
            .target
            .with_output_objects(|| self.authorize_output(|| self.checked_interval()))
    }

    /// Track worker cleanup without granting pixels or extending the issuer lifetime.
    pub(crate) fn track_worker(
        &self,
        revocation: &kernel::drm::capture::Revocation,
    ) -> Result<WorkerRegistration> {
        self.with_output(|| Ok(()))?;
        let permission = self.policy.workers.register(revocation)?;
        let device = self.device().renderer_workers.register(revocation)?;
        self.with_output(|| Ok(()))?;
        Ok(WorkerRegistration {
            _permission: permission,
            _device: device,
        })
    }

    /// Borrow the allocation device without authorizing access to display pixels.
    pub(crate) fn device(&self) -> &Device<Driver> {
        self.policy.permission.target.device()
    }

    pub(crate) fn display(&self) -> &crate::device::Display {
        self.policy.permission.target.display()
    }

    /// Static topology only; no continuing authority follows from this borrow.
    pub(super) fn constraints_output<'a>(
        &'a self,
        registered: &'a Device<Driver, Registered>,
    ) -> Result<kernel::drm::kms::constraints::Output<'a, Driver>> {
        self.policy.permission.target.constraints_output(registered)
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
        self.policy
            .permission
            .target
            .with_output_objects(|| self.authorize_output(f))
    }

    /// Authorize work for one exact uninterrupted master interval.
    pub(crate) fn with_output_interval<R>(
        &self,
        interval: Interval,
        f: impl FnOnce() -> Result<R>,
    ) -> Result<R> {
        self.policy.permission.target.with_output_objects(|| {
            self.authorize_output(|| {
                if self.checked_interval()? != interval {
                    return Err(ESTALE);
                }
                f()
            })
        })
    }

    fn authorize_output<R>(&self, f: impl FnOnce() -> Result<R>) -> Result<R> {
        let _ = self.checked_interval()?;
        let revoked = self.policy.revoked.lock();
        if *revoked {
            return Err(EKEYREVOKED);
        }
        f()
    }

    fn checked_interval(&self) -> Result<Interval> {
        self.device().authority.interval()
    }

    /// Reject current source aliases without requiring enabled video or claiming pixels.
    /// Native master/object locks precede output publication, then permission revocation,
    /// matching source admission even when no scene is currently published.
    pub(super) fn check_private_storage(
        &self,
        interval: Interval,
        buffers: &[kernel::sync::aref::ARef<kernel::dma_buf::DmaBuf>],
    ) -> Result {
        self.policy.permission.target.with_output_objects(|| {
            self.display().output.with_accepted(|accepted| {
                self.authorize_output(|| {
                    if self.checked_interval()? != interval {
                        return Err(ESTALE);
                    }
                    if let Some(scene) = accepted.and_then(|accepted| accepted.scene) {
                        for buffer in buffers {
                            if scene.uses_reservation(buffer.reservation())? {
                                return Err(EINVAL);
                            }
                        }
                    }
                    Ok(())
                })
            })
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

    fn authorize<R>(
        &self,
        current: Current<'_>,
        f: impl FnOnce(Current<'_>) -> Result<R>,
    ) -> Result<R> {
        let _ = self.checked_interval()?;
        let revoked = self.policy.revoked.lock();
        if *revoked {
            return Err(EKEYREVOKED);
        }
        f(current)
    }
}
