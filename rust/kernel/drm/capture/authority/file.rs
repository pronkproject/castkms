// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Revocation file transport over provider-owned authority.

use super::{
    Authority,
    Policy, //
};
use crate::{
    error::from_err_ptr,
    fs::File,
    prelude::*,
    sync::aref::ARef, //
};
use core::{
    ffi::c_void,
    marker::PhantomData,
    ptr::NonNull, //
};

/// A provider lifetime transferred to an anonymous revocation file.
///
/// Final file release drops this owner after authority revocation and outside authority
/// locks. Retaining the owner does not add any capture or pixel operation to the file.
///
/// # Safety
///
/// `OwnerModule` must keep this type's release trampoline, destructor and dependencies
/// executable for the complete retained lifetime. The vtable macro selects the local module.
#[vtable]
pub unsafe trait ControlOwner: Send + 'static {}

struct OwnerCallbacks<O>(PhantomData<O>);

impl<O: ControlOwner> OwnerCallbacks<O> {
    const OPS: bindings::drm_capture_control_owner_ops = bindings::drm_capture_control_owner_ops {
        owner: crate::module::this_module::<O::OwnerModule>().as_ptr(),
        release: Some(Self::release),
    };

    unsafe extern "C" fn release(data: *mut c_void) {
        // SAFETY: Successful file creation owns one KBox<O> and transfers it exactly once
        // on final release. Native code retains O's callback module throughout this call.
        drop(unsafe { KBox::from_raw(data.cast::<O>()) });
    }
}

impl<P: Policy> Authority<P> {
    /// Transfer a provider lifetime to a revocation file without installing a descriptor.
    ///
    /// Cloned file references share the transferred owner. Final release revokes first,
    /// then drops the owner while the file still retains its authority and callback module.
    /// The owner must not retain the file being created, or a reference cycle would result.
    ///
    /// This consuming operation drops `owner` on failure, too. A revoking owner therefore
    /// makes failed construction terminal; use it only after choosing that rollback policy.
    /// Call outside authority admission and every lock needed by the owner's destructor.
    pub fn create_control_file_with_owner<O: ControlOwner>(&self, owner: O) -> Result<ARef<File>> {
        let data = KBox::into_raw(KBox::new(owner, GFP_KERNEL)?);
        // SAFETY: Authority remains live; OPS describes this exact foreign allocation and
        // its module. Native success transfers data; failure leaves it with this caller.
        let result = from_err_ptr(unsafe {
            bindings::drm_capture_control_file_create_owned(
                self.raw.get(),
                &OwnerCallbacks::<O>::OPS,
                data.cast(),
            )
        });
        match result {
            Ok(file) => {
                // SAFETY: Native creation returns one initialized, unpublished file reference,
                // so no concurrent fdget_pos operation violates File's thread-safe invariant.
                Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(file.cast())) })
            }
            Err(error) => {
                // SAFETY: Native failure did not consume the original allocation or call release.
                drop(unsafe { KBox::from_raw(data) });
                Err(error)
            }
        }
    }

    /// Create an owned control file whose final release revokes this authority.
    ///
    /// Cloned file references share one revocation lifetime. Independently created files each
    /// revoke on final release, even while other control files or authority references survive.
    /// File release may be deferred; use [`Self::revoke`] for prompt revocation.
    ///
    /// Creation failure leaves authority unchanged. Successfully created files revoke even if
    /// never published. Issuers must finish fallible setup before publishing descriptors and
    /// treat rollback of an owned file as terminal. Do not release it under an admission guard
    /// or locks needed by provider cleanup.
    ///
    /// The file only reports completed revocation through poll. It grants no image, source,
    /// modesetting or mapping access, and this operation installs no descriptor.
    pub fn create_control_file(&self) -> Result<ARef<File>> {
        // SAFETY: The authority remains live; successful native creation retains its own reference.
        let file =
            from_err_ptr(unsafe { bindings::drm_capture_control_file_create(self.raw.get()) })?;
        // SAFETY: Creation transfers a non-null initialized file reference. The unpublished file
        // has no concurrent fdget_pos operation, satisfying the thread-safe File invariant.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(file.cast())) })
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
