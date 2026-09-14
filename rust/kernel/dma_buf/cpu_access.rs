// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Bounded CPU writes, with exporter mapping and cache-maintenance lifetimes.

use super::DmaBuf;
use crate::{
    error::to_result,
    io::{
        Io,
        SysMem, //
    },
    prelude::*, //
};

/// One CPU write interval over a system-memory DMA-BUF mapping.
///
/// This type does not grant pixel permission or exclude other users of the allocation.
/// The caller must arrange explicit synchronization and recipient authorization separately.
/// Construction may wait for implicit dependencies and takes the buffer reservation lock;
/// call outside source-read claims and locks needed by other buffer users.
///
/// No slice or raw pointer into shared storage escapes. Dropping the interval ends CPU
/// access before unmapping; use [`Self::finish`] to observe cache-maintenance failure
/// before reporting a valid result. An error does not undo bytes already written.
///
/// # Invariants
///
/// While `active`, `map` owns a successful system-memory mapping of `buffer` and a
/// successful `DMA_TO_DEVICE` CPU interval. Inactive owners expose no further byte access.
#[must_use = "finish the CPU write interval before reporting a valid image"]
pub struct Write<'a> {
    buffer: &'a DmaBuf,
    map: bindings::iosys_map,
    active: bool,
}

impl<'a> Write<'a> {
    /// Map ordinary system memory, then begin CPU access, rejecting I/O-memory mappings.
    ///
    /// Read-only exports return `EACCES` before invoking any exporter callbacks. Export
    /// write access does not establish pixel permission or synchronization with other users.
    pub fn new(buffer: &'a DmaBuf) -> Result<Self> {
        if !buffer.is_writable() {
            return Err(EACCES);
        }
        let mut map = bindings::iosys_map::default();
        // SAFETY: The buffer is live and map is exclusive output storage. This unlocked
        // variant acquires the native reservation lock without retaining it on return.
        to_result(unsafe { bindings::dma_buf_vmap_unlocked(buffer.as_raw(), &mut map) })?;
        let begun = if map.is_iomem {
            Err(EOPNOTSUPP)
        } else {
            // SAFETY: The borrowed buffer and its mapping remain live. Mapping first lets
            // the exporter maintain caches for that virtual address during begin and end.
            to_result(unsafe {
                bindings::dma_buf_begin_cpu_access(
                    buffer.as_raw(),
                    bindings::dma_data_direction_DMA_TO_DEVICE,
                )
            })
        };
        if let Err(error) = begun {
            // SAFETY: Mapping succeeded, but no successful CPU interval needs ending.
            unsafe { bindings::dma_buf_vunmap_unlocked(buffer.as_raw(), &mut map) };
            return Err(error);
        }
        Ok(Self {
            buffer,
            map,
            active: true,
        })
    }

    /// Copy only the specified bytes, without changing any surrounding allocation contents.
    pub fn copy_from_slice(&mut self, offset: usize, bytes: &[u8]) -> Result {
        let end = offset.checked_add(bytes.len()).ok_or(EOVERFLOW)?;
        if end > self.buffer.size() {
            return Err(EINVAL);
        }
        if bytes.is_empty() {
            return Ok(());
        }
        // SAFETY: Successful system-memory mapping covers the retained buffer's size.
        // The checked range lies inside that mapping. SysMem exposes I/O operations,
        // not Rust references to externally accessible allocation contents.
        let memory = unsafe {
            SysMem::new(core::ptr::slice_from_raw_parts_mut(
                self.map.__bindgen_anon_1.vaddr.cast::<u8>().add(offset),
                bytes.len(),
            ))
        };
        memory.copy_from_slice(bytes);
        Ok(())
    }

    /// End access exactly once, reporting exporter cache-maintenance errors.
    pub fn finish(mut self) -> Result {
        self.end()
    }

    fn end(&mut self) -> Result {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        // SAFETY: All byte access ended and this matches the successful begin call.
        // Keep the mapping until the exporter has maintained its virtual-address caches.
        let result = to_result(unsafe {
            bindings::dma_buf_end_cpu_access(
                self.buffer.as_raw(),
                bindings::dma_data_direction_DMA_TO_DEVICE,
            )
        });
        // SAFETY: This owner has exactly one successful native mapping. The unlocked
        // unmap variant takes the reservation lock and releases that mapping ownership.
        unsafe { bindings::dma_buf_vunmap_unlocked(self.buffer.as_raw(), &mut self.map) };
        result
    }
}

impl Drop for Write<'_> {
    fn drop(&mut self) {
        let _ = self.end();
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
