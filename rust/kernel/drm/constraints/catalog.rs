// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Bounded native catalogs and independently owned immutable snapshots.

use super::{
    Backend,
    Domain,
    Entry, //
};
use crate::{
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
use core::{
    marker::PhantomData,
    ptr::NonNull,
    slice, //
};

/// Synchronized output catalog; offering an entry never selects it.
///
/// All operations and final release require a context that may sleep. Methods must not be called
/// while holding locks acquired by native validation or acceptance callbacks.
///
/// # Invariants
///
/// Every reference retains a native catalog whose entries were all created with backend type `B`.
#[repr(transparent)]
pub struct Catalog<B: Backend> {
    raw: Opaque<bindings::drm_constraints_catalog>,
    _backend: PhantomData<B>,
}

// SAFETY: Native reference counting and catalog operations are synchronized; B is Send.
unsafe impl<B: Backend> Send for Catalog<B> {}
// SAFETY: Shared operations use native synchronization and all retained backends are Sync.
unsafe impl<B: Backend> Sync for Catalog<B> {}

// SAFETY: Native get/put maintain the initialized catalog and all its retained entries.
unsafe impl<B: Backend> AlwaysRefCounted for Catalog<B> {
    fn inc_ref(&self) {
        // SAFETY: The shared reference retains a live catalog.
        unsafe { bindings::drm_constraints_catalog_get(self.raw.get()) };
    }

    unsafe fn dec_ref(ptr: NonNull<Self>) {
        // SAFETY: The caller transfers a reference to the identically represented native catalog.
        unsafe { bindings::drm_constraints_catalog_put(ptr.as_ptr().cast()) };
    }
}

impl<B: Backend> Catalog<B> {
    /// Create bounded metadata for one output with an initial selected entry.
    ///
    /// This neither attaches a catalog to a CRTC nor validates backend readiness or modesetting
    /// authority. The provider separately establishes those conditions before use.
    pub fn new(domain: &Domain, initial: &Entry<B>, limit: u32) -> Result<ARef<Self>> {
        // SAFETY: Both inputs remain live; native creation retains correctly typed entries.
        let raw = from_err_ptr(unsafe {
            bindings::drm_constraints_catalog_create(domain.0.get(), initial.raw.get(), limit)
        })?;
        // SAFETY: Successful construction transfers a non-null initialized catalog reference.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(raw.cast())) })
    }

    /// Offer an entry without changing accepted selection. Object scope and readiness are
    /// provider responsibilities; native code enforces domain and CRTC identity membership.
    pub fn add(&self, entry: &Entry<B>) -> Result {
        // SAFETY: Both references remain live, and entry has the catalog's backend type.
        to_result(unsafe { bindings::drm_constraints_catalog_add(self.raw.get(), entry.raw.get()) })
    }

    /// Withdraw an offer without changing its immutable meaning or undoing accepted work.
    pub fn withdraw(&self, id: u64) -> Result {
        // SAFETY: Native code serializes the operation against acceptance on a live catalog.
        to_result(unsafe { bindings::drm_constraints_catalog_withdraw(self.raw.get(), id) })
    }

    /// Remove a withdrawn, unselected listing. Outstanding references remain valid.
    pub fn forget(&self, id: u64) -> Result {
        // SAFETY: Native code checks availability and selection before dropping its reference.
        to_result(unsafe { bindings::drm_constraints_catalog_forget(self.raw.get(), id) })
    }

    /// Suggest an available entry, or clear the suggestion with zero. Suggestions never select.
    pub fn suggest(&self, id: u64) -> Result {
        // SAFETY: Native code validates the identity and serializes metadata updates.
        to_result(unsafe { bindings::drm_constraints_catalog_suggest(self.raw.get(), id) })
    }

    /// Permanently exclude new selection and listing, synchronizing with ongoing acceptance.
    ///
    /// Retained entries and snapshots remain valid. Closure is not native completion, source
    /// revocation or permission to restore another contract without quiescing the output.
    pub fn close(&self) {
        // SAFETY: The shared reference retains the catalog throughout synchronized closure.
        unsafe { bindings::drm_constraints_catalog_close(self.raw.get()) };
    }

    /// Retain accepted selection, including after closure. This grants no readiness or authority.
    pub fn selected(&self) -> ARef<Entry<B>> {
        // SAFETY: A live catalog always retains a selected entry of B and returns an owned ref.
        let raw = unsafe { bindings::drm_constraints_catalog_selected(self.raw.get()) };
        // SAFETY: The returned reference is non-null, initialized and has the catalog's type.
        unsafe { ARef::from_raw(NonNull::new_unchecked(raw.cast())) }
    }

    /// Resolve an ID in this output's catalog without reserving subsequent acceptance.
    ///
    /// Zero returns EINVAL. Unknown, withdrawn unselected and closed entries return ESTALE.
    /// A withdrawn selected entry may be retained for repeated selection while the catalog
    /// remains open. Successful lookup grants neither readiness nor modesetting authority.
    pub fn lookup(&self, id: u64) -> Result<ARef<Entry<B>>> {
        // SAFETY: Native lookup synchronizes availability and returns an owned reference to
        // an entry of B retained by this live catalog, or an error without transferring ownership.
        let raw =
            from_err_ptr(unsafe { bindings::drm_constraints_catalog_lookup(self.raw.get(), id) })?;
        // SAFETY: Successful lookup transfers a non-null initialized entry with backend type B.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(raw.cast())) })
    }

    /// Copy a coherent bounded snapshot. Nonzero expected generation must match or returns ESTALE.
    /// A successful snapshot reserves neither availability nor later acceptance.
    pub fn snapshot(&self, generation: u64) -> Result<Snapshot<B>> {
        // SAFETY: The live catalog synchronizes copying and retains each entry for the snapshot.
        let raw = from_err_ptr(unsafe {
            bindings::drm_constraints_catalog_snapshot(self.raw.get(), generation)
        })?;
        Ok(Snapshot {
            // SAFETY: Successful construction transfers unique ownership of a non-null snapshot.
            raw: unsafe { NonNull::new_unchecked(raw) },
            _backend: PhantomData,
        })
    }
}

/// Coherent metadata from one immutable snapshot, not presentation completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotInfo {
    /// Changes with listing, availability, suggestion or accepted selection, not each frame.
    pub generation: u64,
    /// Accepted constraints ID.
    pub selected_id: u64,
    /// Advisory target ID, or zero when none is suggested.
    pub suggested_id: u64,
    /// Number of retained listings.
    pub count: usize,
}

/// Borrowed listing whose backend and description remain alive through the snapshot.
pub struct Offer<'a, B: Backend> {
    /// Retained immutable entry.
    pub entry: &'a Entry<B>,
    /// Availability at snapshot creation; not a promise about subsequent acceptance.
    pub selectable: bool,
}

/// Independently owned immutable list with bounded, correctly typed retained entries.
///
/// Dropping the snapshot may release provider resources and requires sleepable context.
pub struct Snapshot<B: Backend> {
    raw: NonNull<bindings::drm_constraints_snapshot>,
    _backend: PhantomData<B>,
}

// SAFETY: The uniquely owned snapshot is immutable and retains Send backends.
unsafe impl<B: Backend> Send for Snapshot<B> {}
// SAFETY: Shared access only observes immutable metadata and retained Sync entries.
unsafe impl<B: Backend> Sync for Snapshot<B> {}

impl<B: Backend> Drop for Snapshot<B> {
    fn drop(&mut self) {
        // SAFETY: This wrapper uniquely owns the initialized snapshot and releases it once.
        unsafe { bindings::drm_constraints_snapshot_put(self.raw.as_ptr()) };
    }
}

impl<B: Backend> Snapshot<B> {
    /// Copy immutable metadata without consulting the live catalog.
    pub fn info(&self) -> SnapshotInfo {
        // SAFETY: A live snapshot owns initialized immutable metadata for the duration of self.
        let info = unsafe { &*bindings::drm_constraints_snapshot_info(self.raw.as_ptr()) };
        SnapshotInfo {
            generation: info.generation,
            selected_id: info.selected_id,
            suggested_id: info.suggested_id,
            count: info.count as usize,
        }
    }

    /// Iterate immutable listings without retaining or locking the originating catalog.
    pub fn entries(&self) -> impl ExactSizeIterator<Item = Offer<'_, B>> {
        // SAFETY: Native creation owns an initialized bounded array of count listings. The
        // snapshot is immutable and the returned borrow cannot outlive it.
        let entries = unsafe {
            slice::from_raw_parts(
                bindings::drm_constraints_snapshot_entries(self.raw.as_ptr()),
                self.info().count,
            )
        };
        entries.iter().map(|listing| Offer {
            // SAFETY: Each listing owns a non-null initialized entry of the originating catalog's
            // backend type. The snapshot outlives every entry borrow returned by this iterator.
            entry: unsafe { &*listing.entry.cast() },
            selectable: listing.selectable,
        })
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
