// SPDX-License-Identifier: GPL-2.0

//! Read-only references to native DMA completion fences.
//!
//! A completed fence may report failure. Retaining a fence preserves its completion status,
//! not the contents of a buffer, permission to access it, or completion of dependent work.
//! This interface does not create or signal production fences.

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
