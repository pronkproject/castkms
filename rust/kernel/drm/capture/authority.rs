// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Provider policy and revocation ownership, independent of capture file transport.

use crate::{
    error::from_err_ptr,
    prelude::*,
    sync::{
        aref::{
            ARef,
            AlwaysRefCounted, //
        },
        Arc, //
    },
    types::{
        ForeignOwnable,
        Opaque, //
    }, //
};
use core::{
    ffi::c_void,
    marker::PhantomData,
    ptr::NonNull, //
};

/// Provider-owned capture policy and resources.
///
/// The provider retains immutable recipient/scope policy and all resources needed by callbacks.
/// Authority lifetime alone grants no pixel access. Callbacks may sleep, but must not reenter
/// authority operations or reverse the provider's source/policy lock order.
///
/// # Safety
///
/// `OwnerModule` must retain the code and static data used by the policy callbacks and their
/// destruction. The `vtable` implementation attribute selects the implementing module by default.
#[vtable]
pub unsafe trait Policy: Send + Sync + 'static {
    /// Stop new provider resource admission and begin cleanup, outside the authority mutex.
    ///
    /// Called exactly once, after registered streams have been revoked. Submitted work may
    /// remain active with its own ownership; returning does not certify native completion.
    fn revoke(&self);
}

/// Shared authority lifetime retaining its provider and callback module.
///
/// Final reference release revokes the authority. Explicit revocation waits for provider cleanup
/// initiation, not outstanding GPU work. All operations and destruction require sleepable context.
///
/// # Invariants
///
/// Every reference retains an initialized native authority created with `P`'s callbacks and one
/// foreign-owned `Arc<P>`. The native authority holds the callback module until final release.
#[repr(transparent)]
pub struct Authority<P: Policy> {
    raw: Opaque<bindings::drm_capture_authority>,
    _policy: PhantomData<P>,
}

// SAFETY: Native authority references and transitions are synchronized; P supports task transfer.
unsafe impl<P: Policy> Send for Authority<P> {}
// SAFETY: Shared operations use native synchronization and shared provider callbacks require Sync.
unsafe impl<P: Policy> Sync for Authority<P> {}

// SAFETY: Native get/put maintain the initialized authority and its independently retained policy.
unsafe impl<P: Policy> AlwaysRefCounted for Authority<P> {
    fn inc_ref(&self) {
        // SAFETY: The shared reference proves a live initialized native authority.
        unsafe { bindings::drm_capture_authority_get(self.raw.get()) };
    }

    unsafe fn dec_ref(ptr: NonNull<Self>) {
        // SAFETY: The caller transfers one reference to the identically represented authority.
        unsafe { bindings::drm_capture_authority_put(ptr.as_ptr().cast()) };
    }
}

impl<P: Policy> Authority<P> {
    const OPS: bindings::drm_capture_authority_ops = bindings::drm_capture_authority_ops {
        owner: crate::module::this_module::<P::OwnerModule>().as_ptr(),
        revoke: Some(Self::revoke_callback),
        release: Some(Self::release_callback),
        authorize_capture: None,
    };

    unsafe extern "C" fn revoke_callback(data: *mut c_void) {
        // SAFETY: Native lifetime retains the foreign Arc until the later release callback.
        let policy = unsafe { Arc::<P>::borrow(data) };
        policy.revoke();
    }

    unsafe extern "C" fn release_callback(data: *mut c_void) {
        // SAFETY: Final native release transfers its foreign Arc exactly once, after revocation.
        drop(unsafe { Arc::<P>::from_foreign(data) });
    }

    /// Retain a provider's already-established scope and policy without granting source access.
    pub fn new(policy: Arc<P>) -> Result<ARef<Self>> {
        let data = policy.into_foreign();
        // SAFETY: The promoted ops remain valid through P's owning module. The native constructor
        // takes the foreign Arc only on success and retains the module before storing callbacks.
        let raw = unsafe { bindings::drm_capture_authority_create(&Self::OPS, data) };
        let raw = match from_err_ptr(raw) {
            Ok(raw) => raw,
            Err(error) => {
                // SAFETY: Construction failed without consuming or invoking the foreign owner.
                drop(unsafe { Arc::<P>::from_foreign(data) });
                return Err(error);
            }
        };
        // SAFETY: Successful construction transfers one initialized, non-null authority reference.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(raw.cast())) })
    }

    /// Revoke once and wait for the provider's revoke callback to return.
    ///
    /// Do not hold an admission guard or locks needed by the callback. Active jobs retain their
    /// own storage; successful return does not cancel submitted device work.
    pub fn revoke(&self) {
        // SAFETY: The shared reference retains the authority throughout synchronized revocation.
        unsafe { bindings::drm_capture_authority_revoke(self.raw.get()) };
    }

    /// Observe terminal admission closure, which may precede provider callback completion.
    pub fn is_revoked(&self) -> bool {
        // SAFETY: The initialized authority remains live; the native observation is synchronized.
        unsafe { bindings::drm_capture_authority_revoked(self.raw.get()) }
    }

    /// Observe completion of the revoke callback, not native GPU completion.
    pub fn cleanup_done(&self) -> bool {
        // SAFETY: The shared reference retains native completion storage.
        unsafe { bindings::drm_capture_authority_cleanup_done(self.raw.get()) }
    }
}
