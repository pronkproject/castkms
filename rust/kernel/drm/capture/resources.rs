// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Typed resource ownership above the native capture client's name table.

use crate::{
    error::from_err_ptr,
    prelude::*, //
};
use core::ptr::NonNull;

/// Bounded resources with client-local names that are never reused.
///
/// Native code owns the names and slot selection; Rust owns each typed payload.
/// Providers supply authorization and resource limits. Destruction releases all
/// remaining payloads and must run in a context suitable for their destructors.
pub struct Resources<T> {
    names: Names,
    values: KVec<Option<T>>,
}

struct Names(NonNull<bindings::drm_capture_resources>);

// SAFETY: This wrapper uniquely owns the native table; mutation requires an exclusive borrow.
unsafe impl Send for Names {}
// SAFETY: Shared operations only read the table; all native mutation requires exclusive access.
unsafe impl Sync for Names {}

impl Drop for Names {
    fn drop(&mut self) {
        // SAFETY: This wrapper owns the table. Payload cleanup is owned separately by values.
        unsafe { bindings::drm_capture_resources_destroy(self.0.as_ptr()) };
    }
}

fn slot(result: i32) -> Result<usize> {
    if result < 0 {
        Err(Error::from_errno(result))
    } else {
        Ok(result as usize)
    }
}

impl<T> Resources<T> {
    /// Allocate all table slots before any resources are constructed.
    pub fn new(limit: usize) -> Result<Self> {
        // SAFETY: The native constructor accepts a scalar and transfers an owned table.
        let raw = from_err_ptr(unsafe {
            bindings::drm_capture_resources_create(limit.try_into().map_err(|_| EINVAL)?)
        })?;
        let names = Names(NonNull::new(raw).ok_or(ENOMEM)?);
        let mut values = KVec::with_capacity(limit, GFP_KERNEL)?;
        for _ in 0..limit {
            values.push_within_capacity(None).map_err(|_| EIO)?;
        }
        Ok(Self { names, values })
    }

    /// Check whether a name could be inserted, without reserving its name or capacity.
    ///
    /// This observation permits construction outside a caller's table lock. Insertion must
    /// still recheck after reacquiring exclusion; another insertion may invalidate success.
    pub fn check(&self, id: u64) -> Result {
        // SAFETY: The shared borrow retains the table and excludes mutation during inspection.
        slot(unsafe { bindings::drm_capture_resources_check(self.names.0.as_ptr(), id) })?;
        Ok(())
    }

    /// Validate name and capacity before construction; failure leaves the name retryable.
    pub fn insert(&mut self, id: u64, create: impl FnOnce() -> Result<T>) -> Result {
        self.check(id)?;
        let value = create()?;
        // SAFETY: No intervening mutation is possible through the exclusive borrow.
        let index =
            slot(unsafe { bindings::drm_capture_resources_insert(self.names.0.as_ptr(), id) })?;
        self.values[index] = Some(value);
        Ok(())
    }

    /// Borrow a retained resource, without authorizing its use.
    pub fn get(&self, id: u64) -> Result<&T> {
        // SAFETY: The shared borrow excludes native mutation and retains the table.
        let index =
            slot(unsafe { bindings::drm_capture_resources_find(self.names.0.as_ptr(), id) })?;
        self.values[index].as_ref().ok_or(EIO)
    }

    /// Exclusively borrow one retained resource.
    pub fn get_mut(&mut self, id: u64) -> Result<&mut T> {
        // SAFETY: The exclusive borrow retains and stabilizes the native table.
        let index =
            slot(unsafe { bindings::drm_capture_resources_find(self.names.0.as_ptr(), id) })?;
        self.values[index].as_mut().ok_or(EIO)
    }

    /// Transfer cleanup to the caller without making the name available again.
    pub fn remove(&mut self, id: u64) -> Result<T> {
        // SAFETY: Exclusive access permits removal and transfer of the corresponding payload.
        let index =
            slot(unsafe { bindings::drm_capture_resources_remove(self.names.0.as_ptr(), id) })?;
        self.values[index].take().ok_or(EIO)
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
