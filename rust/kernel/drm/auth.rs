// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Retained DRM master identities, without a claim of current display authority.
//!
//! C header: [`include/drm/drm_auth.h`](srctree/include/drm/drm_auth.h)

use super::{Device, Driver};
use crate::{bindings, sync::aref::ARef};
use core::ptr::NonNull;

pub(super) mod callbacks;

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
/// `raw` owns one reference to a live `drm_master` belonging to `_dev`.
pub struct MasterRef<D: Driver> {
    raw: NonNull<bindings::drm_master>,
    _dev: ARef<Device<D>>,
}

// SAFETY: The handle exposes only identity comparison and native atomic reference counting.
// Its retained device remains alive through native master destruction on any thread.
unsafe impl<D: Driver> Send for MasterRef<D> {}
// SAFETY: Shared access neither exposes nor mutates native master fields.
unsafe impl<D: Driver> Sync for MasterRef<D> {}

impl<D: Driver> MasterRef<D> {
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
            _dev: ARef::from(dev),
        }
    }
}

impl<D: Driver> Clone for MasterRef<D> {
    fn clone(&self) -> Self {
        let dev = self._dev.clone();
        // SAFETY: Our reference keeps the master alive while the helper takes another reference.
        unsafe { bindings::drm_master_get(self.raw.as_ptr()) };
        Self {
            raw: self.raw,
            _dev: dev,
        }
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
