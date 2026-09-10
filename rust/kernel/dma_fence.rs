// SPDX-License-Identifier: GPL-2.0

//! Read-only references to native DMA completion fences.
//!
//! A completed fence may report failure. Retaining a fence preserves its completion status,
//! not the contents of a buffer, permission to access it, or completion of dependent work.
//! This interface can combine submitted completion records, but cannot represent future
//! submissions or signal production fences.

use crate::{
    bindings,
    error::to_result,
    prelude::*,
    sync::aref::{
        ARef,
        AlwaysRefCounted, //
    },
    types::Opaque, //
};
use core::ptr::NonNull;

#[cfg(CONFIG_SYNC_FILE)]
mod sync_file;

/// A completion observation, keeping pending work distinct from unsuccessful completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The fence has not signaled.
    Pending,
    /// The fence signaled, with the producer's success or error status.
    Complete(Result),
}

/// A native DMA fence, accessed through its synchronized native operations.
///
/// # Invariants
///
/// The wrapped fence is initialized and remains alive throughout every reference.
#[repr(transparent)]
pub struct Fence(Opaque<bindings::dma_fence>);

// SAFETY: Native reference counting and status inspection are safe across tasks.
unsafe impl Send for Fence {}
// SAFETY: Shared references expose only synchronized native operations.
unsafe impl Sync for Fence {}

// SAFETY: The native get/put protocol owns the allocation behind each reference.
unsafe impl AlwaysRefCounted for Fence {
    fn inc_ref(&self) {
        // SAFETY: A shared reference retains a live native fence.
        unsafe { bindings::dma_fence_get(self.as_raw()) };
    }

    unsafe fn dec_ref(ptr: NonNull<Self>) {
        // SAFETY: The caller transfers one reference to the identically represented fence.
        unsafe { bindings::dma_fence_put(ptr.as_ptr().cast()) };
    }
}

impl Fence {
    /// Combine completion of submitted work without retaining its error history.
    ///
    /// Empty input needs no wait. Native merging flattens containers, omits completed
    /// fences and keeps only the latest pending fence on each timeline. Callers needing
    /// producer validity must retain and inspect the original records independently.
    /// The returned fence does not authorize buffer access or close submission admission.
    pub fn merge_completion(fences: &[ARef<Self>]) -> Result<Option<ARef<Self>>> {
        match fences {
            [] => return Ok(None),
            [fence] => return Ok(Some(fence.clone())),
            _ => {}
        }

        let mut raw = KVec::with_capacity(fences.len(), GFP_KERNEL)?;
        let mut cursors = KVec::with_capacity(fences.len(), GFP_KERNEL)?;
        for fence in fences {
            raw.push(fence.as_raw(), GFP_KERNEL)?;
            cursors.push(Opaque::<bindings::dma_fence_unwrap>::zeroed(), GFP_KERNEL)?;
        }
        // SAFETY: Both arrays have one initialized entry per retained input fence. The
        // native merge borrows those references, initializes and exhausts its cursors,
        // and returns an independently owned reference without nesting containers.
        let merged = unsafe {
            bindings::__dma_fence_unwrap_merge(
                fences.len(),
                raw.as_mut_ptr(),
                cursors.as_mut_ptr().cast(),
            )
        };
        let merged = NonNull::new(merged.cast::<Self>()).ok_or(ENOMEM)?;
        // SAFETY: A non-null merge result transfers one initialized native reference.
        Ok(Some(unsafe { ARef::from_raw(merged) }))
    }

    /// Borrow a native fence while its owner excludes destruction.
    ///
    /// # Safety
    ///
    /// `raw` must name an initialized fence retained throughout the returned lifetime.
    pub(crate) unsafe fn from_raw<'a>(raw: *mut bindings::dma_fence) -> &'a Self {
        // SAFETY: The caller supplies a live allocation with the identical representation.
        unsafe { &*raw.cast() }
    }

    pub(crate) fn as_raw(&self) -> *mut bindings::dma_fence {
        self.0.get()
    }

    /// Inspect completion without waiting or discarding a signaled fence's error.
    ///
    /// Pending is an observation, not a promise that work remains pending after return.
    /// A successful result says nothing about errors in another dependency on its timeline.
    pub fn status(&self) -> Status {
        // SAFETY: The native function serializes inspection of a live fence.
        match unsafe { bindings::dma_fence_get_status(self.as_raw()) } {
            0 => Status::Pending,
            status => Status::Complete(to_result(status)),
        }
    }

    /// Retain the same completion record independently of its current owner.
    pub fn to_owned_ref(&self) -> ARef<Self> {
        self.into()
    }
}

#[cfg(CONFIG_KUNIT)]
pub mod testing;
