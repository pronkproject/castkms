// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Kernel revocation references without access to a provider's capture operations.

use super::{
    Authority,
    Policy, //
};
use crate::{
    bindings,
    sync::aref::{
        ARef,
        AlwaysRefCounted, //
    },
    types::Opaque, //
};
use core::ptr::NonNull;

/// A retained authority reference exposing only revocation and its terminal state.
///
/// The provider type and capture admission operations are deliberately absent. Device
/// shutdown may retain these references without depending on private provider policy.
/// All operations, including final release, require a context that may sleep.
///
/// Ordinary reference release is not an explicit revocation request. The last native
/// authority reference still performs its normal final cleanup; use [`Self::revoke`]
/// when revocation must happen while other owners survive. Do not create a retention
/// cycle: a device tracking its own grants needs a separately owned removal token.
///
/// # Invariants
///
/// Each reference retains an initialized native authority and its original callbacks,
/// policy data and callback module. No conversion reconstructs a typed authority.
#[repr(transparent)]
pub struct Revocation(Opaque<bindings::drm_capture_authority>);

// SAFETY: Native reference counting and revocation are synchronized. Original construction
// requires a thread-safe Policy and retains its module independently of this erased view.
unsafe impl Send for Revocation {}
// SAFETY: All exposed operations use the native authority's synchronized state.
unsafe impl Sync for Revocation {}

// SAFETY: Every reference participates in the original native authority's get/put lifetime.
unsafe impl AlwaysRefCounted for Revocation {
    fn inc_ref(&self) {
        // SAFETY: A shared reference proves an initialized authority with a live reference.
        unsafe { bindings::drm_capture_authority_get(self.0.get()) };
    }

    unsafe fn dec_ref(ptr: NonNull<Self>) {
        // SAFETY: The caller transfers one native reference to the transparent representation.
        unsafe { bindings::drm_capture_authority_put(ptr.as_ptr().cast()) };
    }
}

impl Revocation {
    /// Revoke once and wait for the original provider's revoke callback to return.
    ///
    /// Hold neither an authority admission guard nor locks needed by provider cleanup.
    /// Completion ends admission; it does not cancel previously submitted GPU work.
    pub fn revoke(&self) {
        // SAFETY: The live reference retains the original synchronized native authority.
        unsafe { bindings::drm_capture_authority_revoke(self.0.get()) };
    }

    /// Observe terminal admission closure, which may precede provider callback completion.
    pub fn is_revoked(&self) -> bool {
        // SAFETY: The initialized authority remains live throughout the synchronized read.
        unsafe { bindings::drm_capture_authority_revoked(self.0.get()) }
    }

    /// Observe completion of the provider's revoke callback, not device work completion.
    pub fn cleanup_done(&self) -> bool {
        // SAFETY: The live authority reference retains native completion storage.
        unsafe { bindings::drm_capture_authority_cleanup_done(self.0.get()) }
    }
}

impl<P: Policy> Authority<P> {
    /// Retain revocation access without exposing this authority's provider or capture API.
    pub fn revocation(&self) -> ARef<Revocation> {
        // SAFETY: Revocation transparently represents the same initialized native authority.
        // The borrowed view is live for self's lifetime and conversion acquires its own
        // native reference. No policy cast or replacement callback is involved.
        let revocation = unsafe { &*self.raw.get().cast::<Revocation>() };
        revocation.into()
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
