// SPDX-License-Identifier: GPL-2.0

//! Read-only snapshots of native buffer reservation dependencies.
//!
//! Snapshots retain the completion records acquired by native reservation iteration. They
//! neither prevent subsequent submissions nor recover signaled fences omitted by iteration.
//! In particular, an empty snapshot is not proof of valid pixels or permission to access them.

use crate::{
    bindings,
    dma_fence::Fence,
    error::to_result,
    prelude::*,
    types::Opaque, //
};

/// Native reservation usage, ordered from mandatory kernel work to bookkeeping-only work.
///
/// A snapshot includes usages up to and including the requested value. A new implicit read
/// normally waits for [`Usage::Write`]; a new implicit write waits for [`Usage::Read`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Usage {
    /// Mandatory memory-management dependencies.
    Kernel = bindings::dma_resv_usage_DMA_RESV_USAGE_KERNEL,
    /// Implicit writers, including mandatory kernel dependencies.
    Write = bindings::dma_resv_usage_DMA_RESV_USAGE_WRITE,
    /// Implicit readers, including writers and mandatory kernel dependencies.
    Read = bindings::dma_resv_usage_DMA_RESV_USAGE_READ,
    /// All usages, including work not participating in implicit synchronization.
    Bookkeep = bindings::dma_resv_usage_DMA_RESV_USAGE_BOOKKEEP,
}

/// A borrowed reservation object with read-only access through native synchronization.
///
/// # Invariants
///
/// The native object is initialized and remains live for the entire borrow.
#[repr(transparent)]
pub struct Reservation(Opaque<bindings::dma_resv>);

// SAFETY: The native snapshot operation serializes iteration against concurrent updates.
unsafe impl Sync for Reservation {}
// SAFETY: No thread-local state is exposed, and the borrow retains the native owner.
unsafe impl Send for Reservation {}

impl Reservation {
    /// Borrow an initialized reservation object from its owner.
    ///
    /// # Safety
    ///
    /// `raw` must remain initialized and live for the returned lifetime.
    pub(crate) unsafe fn from_raw<'a>(raw: *mut bindings::dma_resv) -> &'a Self {
        // SAFETY: The caller supplies a live object with the identical representation.
        unsafe { &*raw.cast() }
    }

    /// Retain a native snapshot without taking the reservation's update-side lock.
    ///
    /// May sleep and allocate. Native iteration skips fences observed as signaled, including
    /// failed fences. A collected fence may signal immediately afterward; its own status is
    /// retained. This does not provide an atomic snapshot across multiple reservations.
    pub fn snapshot(&self, usage: Usage) -> Result<Snapshot> {
        let mut snapshot = Snapshot {
            fences: core::ptr::null_mut(),
            count: 0,
        };
        // SAFETY: The owner retains an initialized reservation. The native operation returns
        // an owned kmalloc array of owned references, or clears both outputs on failure.
        to_result(unsafe {
            bindings::dma_resv_get_fences(
                self.0.get(),
                usage as _,
                &mut snapshot.count,
                &mut snapshot.fences,
            )
        })?;
        Ok(snapshot)
    }
}

/// Owned completion records independent of the reservation that supplied them.
///
/// # Invariants
///
/// `fences` is null or a native kmalloc array with at least `count` initialized entries,
/// each owning one non-null fence reference. The unused allocation capacity is not known.
pub struct Snapshot {
    fences: *mut *mut bindings::dma_fence,
    count: u32,
}

// SAFETY: The immutable array owns its references; fences synchronize their shared access.
unsafe impl Send for Snapshot {}
// SAFETY: Iteration only borrows retained, thread-safe fences from an immutable array.
unsafe impl Sync for Snapshot {}

impl Snapshot {
    /// Return the number of acquired completion records.
    pub fn len(&self) -> usize {
        self.count as usize
    }

    /// Return whether native iteration acquired no completion records.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Borrow each acquired record without collapsing timeline identities or error status.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Fence> + '_ {
        (0..self.len()).map(|index| {
            // SAFETY: Each initialized entry owns a reference for the snapshot's lifetime.
            unsafe { Fence::from_raw(*self.fences.add(index)) }
        })
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        for index in 0..self.len() {
            // SAFETY: Release exactly the reference owned by each initialized array entry.
            // No borrowed fence remains accessible after the snapshot is exclusively dropped.
            unsafe { bindings::dma_fence_put(*self.fences.add(index)) };
        }
        // SAFETY: The native array is a kmalloc allocation, not a Rust vector with known
        // capacity. kfree accepts null, including snapshots which acquired no records.
        unsafe { bindings::kfree(self.fences.cast()) };
    }
}

#[cfg(CONFIG_KUNIT)]
pub mod testing;
