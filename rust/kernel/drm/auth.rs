// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Retained DRM master identities and short checks of current object access.
//!
//! C header: [`include/drm/drm_auth.h`](srctree/include/drm/drm_auth.h)

use super::{Device, Driver};
use crate::{bindings, sync::aref::ARef};
use core::ptr::NonNull;

pub(super) mod callbacks;
mod current;

pub use current::CurrentMasterGuard;

/// A file's associated master and current-master status sampled together.
///
/// This is a historical observation, not an authorization token. Master handoff or lease
/// revocation may occur after sampling. Policy for a later operation must account for those
/// transitions independently. A file with no associated master has no snapshot value.
pub struct MasterSnapshot<D: Driver> {
    master: MasterRef<D>,
    was_current: bool,
}

impl<D: Driver> MasterSnapshot<D> {
    pub(super) fn new(master: MasterRef<D>, was_current: bool) -> Self {
        Self {
            master,
            was_current,
        }
    }

    /// The retained identity sampled from the file, not its lessor's identity.
    pub fn master(&self) -> &MasterRef<D> {
        &self.master
    }

    /// Whether the file satisfied DRM's lease-aware current-master predicate when sampled.
    pub fn was_current(&self) -> bool {
        self.was_current
    }
}

/// A retained DRM master identity associated with a file on a primary node.
///
/// A non-master client may share that identity with other clients. Retaining it does not keep
/// the master active, preserve a lease, or authorize capture. Equality compares native identity,
/// not current authority; a lease's master is distinct from its lessor's master.
///
/// The handle retains the device because native master destruction accesses it. Device-owned
/// state must release such handles during shutdown to avoid a reference cycle.
///
/// # Invariants
///
/// `raw` owns one reference to a live `drm_master` belonging to `dev`.
pub struct MasterRef<D: Driver> {
    raw: NonNull<bindings::drm_master>,
    dev: ARef<Device<D>>,
}

// SAFETY: Native references and current-access checks are synchronized. The retained device
// remains alive through native master destruction on any thread.
unsafe impl<D: Driver> Send for MasterRef<D> {}
// SAFETY: Shared methods expose no unguarded native master fields. Current-access guards
// remain on the acquiring task and serialize access using the native locks.
unsafe impl<D: Driver> Sync for MasterRef<D> {}

impl<D: Driver> MasterRef<D> {
    /// Whether this identity was created as a lessee rather than the top-level display owner.
    ///
    /// The relationship does not change when a lease is revoked or its owner loses control.
    /// A false result establishes neither current control nor a file's master role. Providers
    /// that support only top-level ownership can reject lessees independently of access checks.
    pub fn is_lessee(&self) -> bool {
        // SAFETY: The retained initialized master keeps its immutable lessor relationship alive.
        // Native lease construction sets it before publication; only final destruction clears it.
        unsafe { !(*self.raw.as_ptr()).lessor.is_null() }
    }

    /// Take ownership of one native reference, retaining its device independently.
    ///
    /// # Safety
    ///
    /// `raw` must own one reference to an initialized master belonging to `dev`.
    pub(super) unsafe fn from_owned_raw(
        raw: NonNull<bindings::drm_master>,
        dev: &Device<D>,
    ) -> Self {
        Self {
            raw,
            dev: ARef::from(dev),
        }
    }
}

impl<D: Driver> Clone for MasterRef<D> {
    fn clone(&self) -> Self {
        let dev = self.dev.clone();
        // SAFETY: Our reference keeps the master alive while the helper takes another reference.
        unsafe { bindings::drm_master_get(self.raw.as_ptr()) };
        Self { raw: self.raw, dev }
    }
}

impl<D: Driver> PartialEq for MasterRef<D> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl<D: Driver> Eq for MasterRef<D> {}

impl<D: Driver> Drop for MasterRef<D> {
    fn drop(&mut self) {
        let mut raw = self.raw.as_ptr();
        // SAFETY: Release exactly our native reference before the retained device is dropped.
        // The helper clears its pointer argument; the handle is not used after destruction.
        unsafe { bindings::drm_master_put(&mut raw) };
    }
}
