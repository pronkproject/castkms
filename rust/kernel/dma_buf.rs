// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Reference-counted DMA-BUF storage, independent of CPU mapping or device attachment.
//!
//! A reference keeps the allocation alive. It does not make its pixels immutable, establish
//! access synchronization, or revoke access previously granted through another reference.

use crate::{
    bindings,
    error::from_err_ptr,
    prelude::*,
    sync::aref::{ARef, AlwaysRefCounted},
    types::Opaque,
};
use core::ptr::NonNull;

/// A shared DMA buffer whose native file owns its reference count.
///
/// # Invariants
///
/// The wrapped DMA-BUF is initialized and remains alive for the duration of every borrow.
#[repr(transparent)]
pub struct DmaBuf(Opaque<bindings::dma_buf>);

// SAFETY: DMA-BUF references are transferable between tasks. This wrapper exposes neither
// unsynchronized pixel access nor the underlying file's position-dependent operations.
unsafe impl Send for DmaBuf {}
// SAFETY: The exposed metadata is immutable and native reference operations are atomic.
unsafe impl Sync for DmaBuf {}

impl DmaBuf {
    /// Acquire a DMA-BUF reference from a descriptor in the current task's file table.
    pub fn from_fd(fd: i32) -> Result<ARef<Self>> {
        // SAFETY: The native helper validates the descriptor and returns an owned reference.
        let raw = from_err_ptr(unsafe { bindings::dma_buf_get(fd) })?;
        let raw = NonNull::new(raw).ok_or(EINVAL)?;
        // SAFETY: Successful dma_buf_get transfers one initialized DMA-BUF reference.
        Ok(unsafe { Self::from_owned_raw(raw) })
    }

    /// Return the allocation size in bytes, without mapping the allocation.
    pub fn size(&self) -> usize {
        // SAFETY: Published DMA-BUF size is immutable and the borrowed buffer remains live.
        unsafe { (*self.as_raw()).size }
    }

    pub(crate) fn as_raw(&self) -> *mut bindings::dma_buf {
        self.0.get()
    }

    /// Adopt one native reference to an initialized DMA-BUF.
    ///
    /// # Safety
    ///
    /// The pointer must identify a live DMA-BUF with one reference owned by the caller.
    pub(crate) unsafe fn from_owned_raw(raw: NonNull<bindings::dma_buf>) -> ARef<Self> {
        // SAFETY: The caller transfers one reference; repr(transparent) preserves layout.
        unsafe { ARef::from_raw(raw.cast()) }
    }
}

// SAFETY: The native DMA-BUF lifetime is governed by its file reference count. Each increment
// adds one native reference and each decrement releases exactly one such reference.
unsafe impl AlwaysRefCounted for DmaBuf {
    fn inc_ref(&self) {
        // SAFETY: DMA-BUF keeps its immutable file pointer alive. This is get_dma_buf's native
        // reference operation, without exposing file operations or mutable file state.
        unsafe { bindings::get_file((*self.as_raw()).file) };
    }

    unsafe fn dec_ref(object: NonNull<Self>) {
        // SAFETY: The caller transfers one owned DMA-BUF reference to release.
        unsafe { bindings::dma_buf_put(object.cast().as_ptr()) };
    }
}
