// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Capture-client file lifetime without ownership of authority revocation.

use super::{
    Authority,
    Policy, //
};
use crate::{
    drm::capture::Description,
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

/// Client state retained until final release of its anonymous file.
///
/// The file itself does not request revocation. A client owner should release only its own
/// resources, without taking responsibility for revoking the authority shared by siblings.
///
/// # Safety
///
/// `OwnerModule` must retain this type's callback trampolines, destructor and dependencies
/// throughout the transferred lifetime. The vtable macro selects the local module.
#[vtable]
pub unsafe trait ClientOwner: Send + 'static {
    /// Describe a currently authorized offer without allocating images or reading sources.
    ///
    /// Native dispatch serializes calls and retains this owner through their completion.
    /// Check current provider permission even for an unchanged offer. Do not reenter the
    /// same client file; call outside DRM and authority locks. Omitting the method leaves
    /// description queries unsupported.
    fn describe(&mut self) -> Result<Description> {
        Err(EOPNOTSUPP)
    }
}

struct Callbacks<O>(PhantomData<O>);

impl<O: ClientOwner> Callbacks<O> {
    const OPS: bindings::drm_capture_client_owner_ops = bindings::drm_capture_client_owner_ops {
        owner: crate::module::this_module::<O::OwnerModule>().as_ptr(),
        release: Some(Self::release),
        describe: if O::HAS_DESCRIBE {
            Some(Self::describe)
        } else {
            None
        },
    };

    unsafe extern "C" fn release(data: *mut c_void) {
        // SAFETY: Creation transfers one initialized KBox<O> on success. Native final file
        // release returns it exactly once while retaining O's callback module.
        drop(unsafe { KBox::from_raw(data.cast::<O>()) });
    }

    unsafe extern "C" fn describe(
        data: *mut c_void,
        output: *mut bindings::drm_capture_description,
    ) -> i32 {
        // SAFETY: Creation binds the callback to one owned KBox<O>. Native dispatch holds
        // the client's mutex, excluding every other callback, and retains the file so its
        // final release cannot destroy O. No reference to O escapes this call.
        let owner = unsafe { &mut *data.cast::<O>() };
        match owner.describe() {
            Ok(description) => {
                // SAFETY: Native dispatch supplies writable callback-local output storage.
                unsafe { output.write(description.raw()) };
                0
            }
            Err(error) => error.to_errno(),
        }
    }
}

impl<P: Policy> Authority<P> {
    /// Retain one client owner in an anonymous file without installing a descriptor.
    ///
    /// Cloned file references share this owner. Final release destroys it before dropping
    /// the file's ordinary authority reference. Other clients and the independent revocation
    /// owner remain usable; final authority release still performs normal cleanup.
    ///
    /// Failure drops `owner` outside native admission locks, too. Do not retain this file
    /// in its own owner. Call outside every lock needed by owner or authority cleanup.
    /// The file observes completed revocation through poll and supports optional kernel
    /// description queries. It exposes no mapping or primary-node operations, and its
    /// hangup is not device-work completion.
    pub fn create_client_file<O: ClientOwner>(&self, owner: O) -> Result<ARef<File>> {
        let data = KBox::into_raw(KBox::new(owner, GFP_KERNEL)?);
        // SAFETY: Authority remains live. OPS describes the exact allocation and its module;
        // native success consumes data, while failure leaves ownership with this caller.
        let result = from_err_ptr(unsafe {
            bindings::drm_capture_client_file_create(
                self.raw.get(),
                &Callbacks::<O>::OPS,
                data.cast(),
            )
        });
        match result {
            Ok(file) => {
                // SAFETY: Creation returns one initialized, unpublished file reference, without
                // concurrent fdget_pos operations violating the thread-safe File invariant.
                Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(file.cast())) })
            }
            Err(error) => {
                // SAFETY: Failure neither consumes the allocation nor invokes its callback.
                drop(unsafe { KBox::from_raw(data) });
                Err(error)
            }
        }
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
