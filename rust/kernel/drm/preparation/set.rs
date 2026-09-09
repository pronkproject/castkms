// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Owned collections of source admission holds, independent of ticket transport.

use super::Source;
use crate::{
    dma_fence::Fence,
    error::{
        from_err_ptr,
        to_result, //
    },
    prelude::*,
    sync::aref::{
        ARef,
        AlwaysRefCounted, //
    },
    types::Opaque, //
};
use core::ptr::NonNull;

/// An immutable collection of admission holds for sources in one domain.
///
/// Duplicate source identities are coalesced. Dropping the final owner releases only this
/// set's holds; existing readers and other holds remain accounted for. The provider retains
/// pixel storage, authority and the correspondence between sources and a display update.
/// All operations, including final reference release, require a context that may sleep.
///
/// # Invariants
///
/// Every reference retains an initialized native set owning all of its admission holds.
#[repr(transparent)]
pub struct RetirementSet(Opaque<bindings::drm_prepare_retirement_set>);

// SAFETY: The immutable native set supports thread-safe references and synchronized hold release.
unsafe impl Send for RetirementSet {}
// SAFETY: Shared access cannot mutate the collection or release another owner's reference.
unsafe impl Sync for RetirementSet {}

// SAFETY: Native get/put retain and release the initialized set allocation.
unsafe impl AlwaysRefCounted for RetirementSet {
    fn inc_ref(&self) {
        // SAFETY: The shared reference retains an initialized native set.
        unsafe { bindings::drm_prepare_retirement_set_get(self.0.get()) };
    }

    unsafe fn dec_ref(ptr: NonNull<Self>) {
        // SAFETY: The caller transfers one reference to the identically represented native set.
        unsafe { bindings::drm_prepare_retirement_set_put(ptr.as_ptr().cast()) };
    }
}

impl RetirementSet {
    pub(super) fn as_raw(&self) -> *mut bindings::drm_prepare_retirement_set {
        self.0.get()
    }

    /// Hold admission for all sources atomically, or leave no holds on failure.
    ///
    /// Empty sets are valid. Mixed domains report EXDEV. Admission ownership neither resolves
    /// outstanding claims nor establishes readiness for a display commit.
    pub fn new(sources: &[ARef<Source>]) -> Result<ARef<Self>> {
        let count = sources.len().try_into().map_err(|_| EOVERFLOW)?;
        let mut raw = KVec::with_capacity(sources.len(), GFP_KERNEL)?;
        for source in sources {
            raw.push(source.0.get(), GFP_KERNEL)?;
        }
        // SAFETY: The borrowed slice retains every source, and raw supplies count initialized
        // pointers. Native creation copies the array and retains its own source references.
        let set = from_err_ptr(unsafe {
            bindings::drm_prepare_retirement_set_create(raw.as_ptr(), count)
        })?;
        // SAFETY: Successful creation transfers a non-null initialized set reference.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(set.cast())) })
    }

    /// Retain proof that all admitted claims have been relinquished for every member.
    ///
    /// `None` means claims remain pending; abandonment reports terminal EIO even if another
    /// member is still pending. Successful preparation does not mean GPU reads have completed.
    pub fn prepared(&self) -> Result<Option<PreparedRetirement>> {
        // SAFETY: The set remains live and native readiness inspects retained holds.
        match to_result(unsafe { bindings::drm_prepare_retirement_set_ready(self.0.get()) }) {
            Err(EAGAIN) => Ok(None),
            Err(error) => Err(error),
            Ok(()) => Ok(Some(PreparedRetirement { set: self.into() })),
        }
    }

    /// Wait interruptibly for admitted claims to be relinquished and retain the ready set.
    ///
    /// Native readers may still be executing. Signals return ERESTARTSYS and an abandoned
    /// claim returns EIO without releasing this owner's admission holds. Do not hold display,
    /// provider or other locks needed by claim owners. Canceling a separate ticket does not
    /// cancel a wait on this independently owned set.
    pub fn wait_prepared(&self) -> Result<PreparedRetirement> {
        // SAFETY: The shared reference retains every hold and its native wait queue. Readiness
        // remains established under those holds after the interruptible wait succeeds.
        to_result(unsafe { bindings::drm_prepare_retirement_set_wait(self.0.get()) })?;
        Ok(PreparedRetirement { set: self.into() })
    }
}

/// A retained, fixed set of submitted readers, not GPU completion or an accepted display update.
///
/// The owned set keeps admission closed for every member. Dropping another set reference cannot
/// invalidate this preparation proof. Pixel storage and display-state validation remain external.
pub struct PreparedRetirement {
    set: ARef<RetirementSet>,
}

impl PreparedRetirement {
    pub(super) fn as_raw_set(&self) -> *mut bindings::drm_prepare_retirement_set {
        self.set.0.get()
    }

    /// Retain native completion of every submitted reader without waiting for future submission.
    ///
    /// The returned fence remains valid after the prepared owner is dropped, but retaining the
    /// fence alone does not keep admission closed. Fence errors end access without proving pixels
    /// valid. No fence is needed for an empty set or a set with only synchronous readers.
    pub fn completion(&self) -> Result<Option<ARef<Fence>>> {
        let mut fence = core::ptr::null_mut();
        // SAFETY: The owned set retains all admission holds; the output pointer is writable.
        to_result(unsafe {
            bindings::drm_prepare_retirement_set_completion(self.set.0.get(), &mut fence)
        })?;
        // SAFETY: Successful completion transfers one reference for a non-null native fence.
        Ok(NonNull::new(fence.cast::<Fence>()).map(|raw| unsafe { ARef::from_raw(raw) }))
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
