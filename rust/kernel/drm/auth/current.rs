// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Short native checks of current master identity and modesetting object access.

use super::MasterRef;
use crate::{
    bindings,
    drm::{
        file::File,
        kms::{
            KmsDriver,
            ModeObject, //
        }, //
    },
    types::NotThreadSafe, //
};

/// Current master identity and lease membership stabilized on the acquiring task.
///
/// The guard holds DRM's master mutex followed by its object-ID mutex. It does not establish
/// device registration, output enablement, a file's master role, or permission to capture pixels.
/// An empty or revoked lease may have a current root; check every required object explicitly.
///
/// Keep the critical section short. Do not acquire modesetting locks, remove objects, close DRM
/// files or release final master/object references while holding it. Do not wait for rendering,
/// worker completion or consumer buffer reuse. A caller must account for any further policy locks
/// in the same order used by master changes and object removal.
///
/// # Invariants
///
/// The current task owns the native locks acquired for `master`. The borrowed identity keeps
/// both the master and its device alive until those locks are released.
///
/// The guard cannot move to another task:
///
/// ```compile_fail
/// use kernel::drm::{auth::CurrentMasterGuard, kms::KmsDriver};
/// fn requires_send<T: Send>() {}
/// fn move_guard<D: KmsDriver>() {
///     requires_send::<CurrentMasterGuard<'_, D>>();
/// }
/// ```
///
/// Nor can it outlive the retained identity:
///
/// ```compile_fail
/// use kernel::drm::{auth::{CurrentMasterGuard, MasterRef}, kms::KmsDriver};
/// fn escape<'a, D: KmsDriver>(master: MasterRef<D>) -> Option<CurrentMasterGuard<'a, D>> {
///     master.lock_current()
/// }
/// ```
#[must_use]
pub struct CurrentMasterGuard<'a, D: KmsDriver> {
    master: &'a MasterRef<D>,
    _task: NotThreadSafe,
}

impl<D: KmsDriver> MasterRef<D> {
    /// Stabilize the identity's current lease root and its object access.
    ///
    /// May sleep. The caller must not hold the device's master or object-ID mutex. In
    /// particular, do not call from a native master transition callback. Returns `None` if the
    /// lease root is no longer current, without retaining either lock.
    pub fn lock_current(&self) -> Option<CurrentMasterGuard<'_, D>> {
        // SAFETY: Our reference retains the initialized master and device throughout acquisition.
        if !unsafe { bindings::drm_master_lock_current(self.raw.as_ptr()) } {
            return None;
        }
        // INVARIANT: The native helper acquired both locks on this task; the borrowed master
        // remains alive until the non-transferable guard releases them in Drop.
        Some(CurrentMasterGuard {
            master: self,
            _task: NotThreadSafe,
        })
    }
}

impl<D: KmsDriver> CurrentMasterGuard<'_, D> {
    /// Identity whose current control is stabilized by this guard.
    ///
    /// A lease retains its own identity, not its lessor's. Cloning the returned handle
    /// preserves identity and storage lifetime, but does not retain the guard or authority.
    pub fn master(&self) -> &MasterRef<D> {
        self.master
    }

    /// Whether the file owns the exact master identity stabilized by this guard.
    ///
    /// Merely sharing that identity is insufficient. Another device or a different identity
    /// under the same lease root is rejected. The result remains stable while the guard is
    /// held, but establishes neither object access nor permission to create a capture grant.
    pub fn is_master_file(&self, file: &File<D::File>) -> bool {
        if file.device_raw() != self.master.dev.as_raw() {
            return false;
        }
        // SAFETY: The same device's master mutex protects the live file's role and association.
        // Both fields remain stable under the guard; no foreign device's fields are inspected.
        unsafe { (*file.as_raw()).is_master && (*file.as_raw()).master == self.master.raw.as_ptr() }
    }

    /// Check that the same object remains registered and covered by the native lease.
    ///
    /// Reject another device without reading its object ID. Capture scope must still identify
    /// its intended objects; native lease membership alone is not a capture grant.
    pub fn holds_object<O: ModeObject<Driver = D>>(&self, object: &O) -> bool {
        if self.master.dev.as_raw() != object.drm_dev().as_raw() {
            return false;
        }
        // SAFETY: The object belongs to our live device and remains borrowed during the check.
        // The guard holds both native locks, including the lock protecting object registration.
        unsafe {
            bindings::drm_master_holds_object_locked(
                self.master.raw.as_ptr(),
                object.raw_mode_obj(),
            )
        }
    }
}

impl<D: KmsDriver> Drop for CurrentMasterGuard<'_, D> {
    fn drop(&mut self) {
        // SAFETY: The guard owns one successful lock acquisition on this task. Its borrowed
        // identity retains the master and device through the matching unlock.
        unsafe { bindings::drm_master_unlock_current(self.master.raw.as_ptr()) };
    }
}
