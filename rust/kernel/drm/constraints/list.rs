// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Bounded native lists and independently owned immutable snapshots.

use super::{
    Domain,
    OpaqueEntry, //
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
    ptr::NonNull,
    slice, //
};

/// Synchronized output list; offering an entry never selects it.
///
/// All operations and final release require a context that may sleep. Methods must not be called
/// while holding locks acquired by native validation or acceptance callbacks.
///
/// # Invariants
///
/// Every reference retains an initialized native list and its independently owned entries.
#[repr(transparent)]
pub struct List {
    raw: Opaque<bindings::drm_constraints_list>,
}

// SAFETY: Native reference counting and list operations are synchronized.
unsafe impl Send for List {}
// SAFETY: Shared operations synchronize metadata and expose no private backend contexts.
unsafe impl Sync for List {}

// SAFETY: Native get/put maintain the initialized list and all its retained entries.
unsafe impl AlwaysRefCounted for List {
    fn inc_ref(&self) {
        // SAFETY: The shared reference retains a live list.
        unsafe { bindings::drm_constraints_list_get(self.raw.get()) };
    }

    unsafe fn dec_ref(ptr: NonNull<Self>) {
        // SAFETY: The caller transfers a reference to the identically represented native list.
        unsafe { bindings::drm_constraints_list_put(ptr.as_ptr().cast()) };
    }
}

impl List {
    /// Maximum number of entries supported by one native constraints list.
    pub const MAX_ENTRIES: usize = bindings::DRM_CONSTRAINTS_MAX_ENTRIES as usize;

    /// Create bounded metadata for one output with an initial selected entry.
    ///
    /// This neither attaches a list to a CRTC nor validates backend readiness or modesetting
    /// authority. The provider separately establishes those conditions before use.
    pub fn new(domain: &Domain, initial: &OpaqueEntry, limit: u32) -> Result<ARef<Self>> {
        // SAFETY: Both inputs remain live; native creation retains the initialized entry.
        let raw = from_err_ptr(unsafe {
            bindings::drm_constraints_list_create(domain.0.get(), initial.as_raw(), limit)
        })?;
        // SAFETY: Successful construction transfers a non-null initialized list reference.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(raw.cast())) })
    }

    /// Offer an entry without changing accepted selection. Object scope and readiness are
    /// provider responsibilities; native code enforces domain and CRTC identity membership.
    pub fn add(&self, entry: &OpaqueEntry) -> Result {
        // SAFETY: Both references remain live; native code retains the entry on success.
        to_result(unsafe { bindings::drm_constraints_list_add(self.raw.get(), entry.as_raw()) })
    }

    /// Withdraw an offer without changing its immutable meaning or undoing accepted work.
    pub fn withdraw(&self, id: u64) -> Result {
        // SAFETY: Native code serializes the operation against acceptance on a live list.
        to_result(unsafe { bindings::drm_constraints_list_withdraw(self.raw.get(), id) })
    }

    /// Remove a withdrawn, unselected listing. Outstanding references remain valid.
    pub fn forget(&self, id: u64) -> Result {
        // SAFETY: Native code checks availability and selection before dropping its reference.
        to_result(unsafe { bindings::drm_constraints_list_forget(self.raw.get(), id) })
    }

    /// Suggest an available entry, or clear the suggestion with zero. Suggestions never select.
    pub fn suggest(&self, id: u64) -> Result {
        // SAFETY: Native code validates the identity and serializes metadata updates.
        to_result(unsafe { bindings::drm_constraints_list_suggest(self.raw.get(), id) })
    }

    /// Permanently exclude new selection and listing, synchronizing with ongoing acceptance.
    ///
    /// Retained entries and snapshots remain valid. Closure is not native completion, source
    /// revocation or permission to restore another contract without quiescing the output.
    pub fn close(&self) {
        // SAFETY: The shared reference retains the list throughout synchronized closure.
        unsafe { bindings::drm_constraints_list_close(self.raw.get()) };
    }

    /// Retain accepted selection, including after closure. This grants no readiness or authority.
    pub fn selected(&self) -> ARef<OpaqueEntry> {
        // SAFETY: A live list always retains a selected entry and returns an owned reference.
        let raw = unsafe { bindings::drm_constraints_list_selected(self.raw.get()) };
        // SAFETY: The returned reference is non-null and initialized; the view is transparent.
        unsafe { ARef::from_raw(NonNull::new_unchecked(raw.cast())) }
    }

    /// Resolve an ID in this output's list without reserving subsequent acceptance.
    ///
    /// Zero returns EINVAL. Unknown, withdrawn unselected and closed entries return ESTALE.
    /// A withdrawn selected entry may be retained for repeated selection while the list
    /// remains open. Successful lookup grants neither readiness nor modesetting authority.
    pub fn lookup(&self, id: u64) -> Result<ARef<OpaqueEntry>> {
        // SAFETY: Native lookup synchronizes availability and returns an owned reference to
        // an entry retained by this live list, or an error without transferring ownership.
        let raw =
            from_err_ptr(unsafe { bindings::drm_constraints_list_lookup(self.raw.get(), id) })?;
        // SAFETY: Successful lookup transfers a non-null initialized entry with transparent layout.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(raw.cast())) })
    }

    /// Copy a coherent bounded snapshot. Nonzero expected generation must match or returns ESTALE.
    /// A successful snapshot reserves neither availability nor later acceptance.
    pub fn snapshot(&self, generation: u64) -> Result<Snapshot> {
        // SAFETY: The live list synchronizes copying and retains each entry for the snapshot.
        let raw = from_err_ptr(unsafe {
            bindings::drm_constraints_list_snapshot(self.raw.get(), generation)
        })?;
        Ok(Snapshot {
            // SAFETY: Successful construction transfers unique ownership of a non-null snapshot.
            raw: unsafe { NonNull::new_unchecked(raw) },
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
pub struct Offer<'a> {
    /// Retained immutable entry.
    pub entry: &'a OpaqueEntry,
    /// Availability at snapshot creation; not a promise about subsequent acceptance.
    pub selectable: bool,
}

/// Independently owned immutable snapshot with bounded retained entries of any backend type.
///
/// Dropping the snapshot may release provider resources and requires sleepable context.
pub struct Snapshot {
    raw: NonNull<bindings::drm_constraints_snapshot>,
}

// SAFETY: The uniquely owned snapshot is immutable and retains thread-safe native entry references.
unsafe impl Send for Snapshot {}
// SAFETY: Shared access observes immutable metadata, never provider-private data.
unsafe impl Sync for Snapshot {}

impl Drop for Snapshot {
    fn drop(&mut self) {
        // SAFETY: This wrapper uniquely owns the initialized snapshot and releases it once.
        unsafe { bindings::drm_constraints_snapshot_put(self.raw.as_ptr()) };
    }
}

impl Snapshot {
    /// Encode the retained snapshot into independently owned kernel bytes.
    ///
    /// The common UAPI description records use native DRM meanings and contain no pointers or
    /// retained resources. These bytes do not implement a userspace ioctl or confer authority.
    /// Allocation may use virtual memory for large lists and requires sleepable context.
    pub fn encode(&self) -> Result<KVVec<u8>> {
        let mut required = 0;
        // SAFETY: The live immutable snapshot permits size discovery with a null/zero buffer;
        // required is a separate writable scalar and no buffer is accessed.
        to_result(unsafe {
            bindings::drm_constraints_snapshot_encode(
                self.raw.as_ptr(),
                core::ptr::null_mut(),
                0,
                &mut required,
            )
        })?;
        let mut bytes = KVVec::<u8>::with_capacity(required, GFP_KERNEL)?;
        // SAFETY: The allocation has at least required writable bytes and overlaps neither the
        // snapshot nor the size output. Immutable metadata keeps the discovered size unchanged.
        to_result(unsafe {
            bindings::drm_constraints_snapshot_encode(
                self.raw.as_ptr(),
                bytes.as_mut_ptr().cast(),
                required,
                &mut required,
            )
        })?;
        // SAFETY: Successful encoding initialized exactly required bytes within the reserved
        // capacity. Byte values have no invalid bit patterns; the vector's length was zero.
        unsafe { bytes.inc_len(required) };
        Ok(bytes)
    }

    /// Copy immutable metadata without consulting the live list.
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

    /// Iterate immutable listings without retaining or locking the originating list.
    pub fn entries(&self) -> impl ExactSizeIterator<Item = Offer<'_>> {
        // SAFETY: Native creation owns an initialized bounded array of count listings. The
        // snapshot is immutable and the returned borrow cannot outlive it.
        let entries = unsafe {
            slice::from_raw_parts(
                bindings::drm_constraints_snapshot_entries(self.raw.as_ptr()),
                self.info().count,
            )
        };
        entries.iter().map(|listing| Offer {
            // SAFETY: Each listing owns a non-null initialized native entry. The transparent
            // opaque view borrows that reference no longer than the snapshot's lifetime.
            entry: unsafe { &*listing.entry.cast() },
            selectable: listing.selectable,
        })
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
