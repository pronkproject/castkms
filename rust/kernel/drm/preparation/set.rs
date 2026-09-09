// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Owned collections of source admission holds, independent of ticket transport.

use super::Source;
use crate::{
    error::from_err_ptr,
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
}

#[cfg(CONFIG_KUNIT)]
mod tests;
