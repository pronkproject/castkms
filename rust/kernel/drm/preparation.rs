// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Source-generation read accounting, separate from pixel authority and atomic tickets.
//!
//! The provider retains storage and authorizes each read independently. These objects do not
//! hold KMS state, grant pixel access, or freeze buffer contents. All operations may sleep.

use crate::{
    dma_fence::Fence,
    error::{from_err_ptr, to_result},
    prelude::*,
    sync::aref::{ARef, AlwaysRefCounted},
    types::Opaque,
};
use core::{mem::ManuallyDrop, ptr::NonNull};

mod domain;
mod guard;
mod set;

pub use self::{
    domain::Domain,
    guard::RetirementGuard,
    set::{
        PreparedRetirement,
        RetirementSet, //
    }, //
};

/// Bounded admission for one source generation.
///
/// [`Self::seal`] closes admission permanently; [`Self::hold_admission`] owns an admission hold.
/// Neither operation creates a multi-output preparation ticket.
/// Capacity includes unresolved claims and submitted reads that have not completed.
///
/// # Invariants
///
/// The native source is initialized and retained throughout every reference.
#[repr(transparent)]
pub struct Source(Opaque<bindings::drm_prepare_source>);

// SAFETY: Native reference counting and accounting operations are serialized.
unsafe impl Send for Source {}
// SAFETY: Shared access exposes only synchronized native operations.
unsafe impl Sync for Source {}

// SAFETY: Native get/put govern the initialized allocation's reference count.
unsafe impl AlwaysRefCounted for Source {
    fn inc_ref(&self) {
        // SAFETY: The shared reference retains a live native source.
        unsafe { bindings::drm_prepare_source_get(self.0.get()) };
    }

    unsafe fn dec_ref(ptr: NonNull<Self>) {
        // SAFETY: The caller transfers one reference to the identically represented source.
        unsafe { bindings::drm_prepare_source_put(ptr.as_ptr().cast()) };
    }
}

impl Source {
    /// Allocate bounded source-generation accounting in a private admission domain.
    ///
    /// This does not grant source access. Sources prepared together use [`Self::new_in`].
    pub fn new(capacity: u32) -> Result<ARef<Self>> {
        // SAFETY: Creation accepts a scalar capacity and returns an owned reference or error.
        let raw = from_err_ptr(unsafe { bindings::drm_prepare_source_create(capacity) })?;
        // SAFETY: Successful creation returns a non-null initialized native allocation.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(raw.cast())) })
    }

    /// Allocate a source in a shared admission domain without granting pixel access.
    pub fn new_in(domain: &Domain, capacity: u32) -> Result<ARef<Self>> {
        // SAFETY: The borrowed domain remains live while native creation takes its reference.
        let raw = from_err_ptr(unsafe {
            bindings::drm_prepare_source_create_in(domain.0.get(), capacity)
        })?;
        // SAFETY: Successful creation transfers one initialized source reference.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(raw.cast())) })
    }

    /// Admit a read after the provider has authorized it and secured independent destination
    /// capacity. The returned claim must not wait for downstream buffer reuse.
    pub fn claim(&self) -> Result<ReadClaim> {
        // SAFETY: The source is retained and the native core serializes admission with hold.
        let raw = from_err_ptr(unsafe { bindings::drm_prepare_source_claim(self.0.get()) })?;
        Ok(ReadClaim {
            // SAFETY: A successful claim returns a unique non-null claim owner.
            raw: unsafe { NonNull::new_unchecked(raw) },
        })
    }

    /// Permanently close admission, leaving already-claimed access unresolved.
    pub fn seal(&self) {
        // SAFETY: The source is live; sealing is synchronized and idempotent.
        unsafe { bindings::drm_prepare_source_seal(self.0.get()) };
    }

    /// Block new read claims while an admission hold or its prepared owner remains alive.
    /// Final release permits admission again unless another hold or permanent seal remains.
    pub fn hold_admission(&self) -> Result<ARef<AdmissionHold>> {
        // SAFETY: The source is live and native creation atomically records an admission hold.
        let raw =
            from_err_ptr(unsafe { bindings::drm_prepare_source_hold_admission(self.0.get()) })?;
        // SAFETY: Successful creation transfers one initialized admission-hold reference.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(raw.cast())) })
    }

    /// Retain proof that admission is closed and every claim has been relinquished.
    ///
    /// `None` means preparation remains pending. Abandonment reports terminal EIO.
    /// Successful preparation requires a permanent seal, not completion of native reads.
    /// Admission-hold owners use [`AdmissionHold::prepared`] to retain their own admission hold.
    pub fn prepared(&self) -> Result<Option<PreparedSource>> {
        // SAFETY: The source is live and readiness is inspected under the native lock.
        match to_result(unsafe { bindings::drm_prepare_source_ready(self.0.get()) }) {
            Err(EAGAIN) => Ok(None),
            Err(error) => Err(error),
            Ok(()) => Ok(Some(PreparedSource {
                admission_closure: AdmissionClosure::PermanentlySealed(self.into()),
            })),
        }
    }
}

/// An owned hold on new read admission, independent of any ticket file descriptor.
///
/// Holding admission does not pause GPU execution, stop existing readers, or freeze pixels.
///
/// # Invariants
///
/// The native hold remains initialized and retains its source throughout every reference.
#[repr(transparent)]
pub struct AdmissionHold(Opaque<bindings::drm_prepare_admission_hold>);

// SAFETY: Native references and source accounting support cross-task ownership.
unsafe impl Send for AdmissionHold {}
// SAFETY: Shared methods inspect synchronized state without releasing the owned hold.
unsafe impl Sync for AdmissionHold {}

// SAFETY: Native get/put maintain the admission-hold allocation and its source reference.
unsafe impl AlwaysRefCounted for AdmissionHold {
    fn inc_ref(&self) {
        // SAFETY: A shared reference proves the native admission hold remains alive.
        unsafe { bindings::drm_prepare_admission_hold_get(self.0.get()) };
    }

    unsafe fn dec_ref(ptr: NonNull<Self>) {
        // SAFETY: Consume the caller's reference to the identically represented native hold.
        unsafe { bindings::drm_prepare_admission_hold_put(ptr.as_ptr().cast()) };
    }
}

impl AdmissionHold {
    /// Retain this admission hold once all admitted readers have relinquished their claims.
    /// Dropping another reference cannot invalidate the returned owner's preparation proof.
    pub fn prepared(&self) -> Result<Option<PreparedSource>> {
        // SAFETY: The retained hold excludes reopening while native readiness is inspected.
        match to_result(unsafe { bindings::drm_prepare_admission_hold_ready(self.0.get()) }) {
            Err(EAGAIN) => Ok(None),
            Err(error) => Err(error),
            Ok(()) => Ok(Some(PreparedSource {
                admission_closure: AdmissionClosure::Held(self.into()),
            })),
        }
    }
}

/// Unique ownership of an admitted source-read claim, not evidence that access has occurred.
///
/// Dropping an unresolved claim marks terminal service failure, not completed access.
/// The provider remains responsible for best-effort cleanup of unreported native work.
#[must_use = "an unresolved read must be released or its source preparation fails"]
pub struct ReadClaim {
    raw: NonNull<bindings::drm_prepare_read_claim>,
}

// SAFETY: Unique claim ownership can move across tasks; native transitions are locked.
unsafe impl Send for ReadClaim {}

impl ReadClaim {
    /// Promise that no access occurred, or all synchronous CPU reads have ended, and no
    /// further access will be performed under this claim.
    pub fn release_cpu(self) {
        self.release(None);
    }

    /// Relinquish access with native completion covering all submitted reads.
    ///
    /// The trusted provider promises no further submissions under this claim. Completion
    /// must not depend on userspace submitting more work or on downstream destination reuse.
    pub fn release_submitted(self, fence: &Fence) {
        self.release(Some(fence));
    }

    fn release(self, fence: Option<&Fence>) {
        let read = ManuallyDrop::new(self);
        // SAFETY: Consume the unique native claim once. The borrowed fence is live for native
        // reference acquisition; ManuallyDrop prevents subsequent abandonment of that claim.
        unsafe {
            bindings::drm_prepare_read_release(
                read.raw.as_ptr(),
                fence.map_or(core::ptr::null_mut(), Fence::as_raw),
            )
        };
    }
}

impl Drop for ReadClaim {
    fn drop(&mut self) {
        // SAFETY: An unreleased unique claim remains owned and keeps its source alive.
        unsafe { bindings::drm_prepare_read_abandon(self.raw.as_ptr()) };
    }
}

/// A source with admission closed and all claims resolved, not completed GPU work or a KMS commit.
///
/// No claims remain and admission cannot reopen while this owner exists, so its native
/// completion set cannot grow.
pub struct PreparedSource {
    admission_closure: AdmissionClosure,
}

enum AdmissionClosure {
    PermanentlySealed(ARef<Source>),
    Held(ARef<AdmissionHold>),
}

impl PreparedSource {
    /// Retain completion of already-submitted readers. An absent fence needs no native wait.
    /// Fence errors establish ended access, not a valid captured image.
    pub fn completion(&self) -> Result<Option<ARef<Fence>>> {
        let mut fence = core::ptr::null_mut();
        // SAFETY: The prepared owner retains admission closure; the output is writable.
        let result = unsafe {
            match &self.admission_closure {
                AdmissionClosure::PermanentlySealed(source) => {
                    bindings::drm_prepare_source_completion(source.0.get(), &mut fence)
                }
                AdmissionClosure::Held(hold) => {
                    bindings::drm_prepare_admission_hold_completion(hold.0.get(), &mut fence)
                }
            }
        };
        to_result(result)?;
        // SAFETY: A non-null successful result transfers one native fence reference.
        Ok(NonNull::new(fence.cast::<Fence>()).map(|raw| unsafe { ARef::from_raw(raw) }))
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
