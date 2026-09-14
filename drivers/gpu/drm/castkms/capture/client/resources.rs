// SPDX-License-Identifier: GPL-2.0-only

//! Bounded client resources named by caller-supplied, non-reusable numbers.

use kernel::prelude::*;

struct Entry<T> {
    id: u64,
    value: T,
}

/// Own resources without keeping a history of removed entries.
///
/// Successful insertion advances the high-water mark; removal never lowers it.
/// The caller supplies authorization and cleanup semantics for T. This table
/// does not establish permission or synchronize operations on the resources.
pub(super) struct Resources<T> {
    entries: KVec<Entry<T>>,
    limit: usize,
    last_id: u64,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl<T> Resources<T> {
    pub(super) fn new(limit: usize) -> Result<Self> {
        if limit == 0 {
            return Err(EINVAL);
        }
        Ok(Self {
            entries: KVec::with_capacity(limit, GFP_KERNEL)?,
            limit,
            last_id: 0,
        })
    }

    /// Validate the name and capacity before constructing a resource.
    ///
    /// The table reserves every slot at creation, so insertion needs no allocation
    /// after the constructor succeeds. Failure leaves the high-water mark unchanged.
    pub(super) fn insert(&mut self, id: u64, create: impl FnOnce() -> Result<T>) -> Result {
        if id == 0 {
            return Err(EINVAL);
        }
        if self.last_id == u64::MAX {
            return Err(EOVERFLOW);
        }
        if id <= self.last_id {
            return Err(ESTALE);
        }
        if self.entries.len() == self.limit {
            return Err(EBUSY);
        }
        let value = create()?;
        self.entries
            .push_within_capacity(Entry { id, value })
            .map_err(|_| EIO)?;
        self.last_id = id;
        Ok(())
    }

    pub(super) fn get_mut(&mut self, id: u64) -> Result<&mut T> {
        if id == 0 {
            return Err(EINVAL);
        }
        self.entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .map(|entry| &mut entry.value)
            .ok_or(ENOENT)
    }

    pub(super) fn get(&self, id: u64) -> Result<&T> {
        if id == 0 {
            return Err(EINVAL);
        }
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| &entry.value)
            .ok_or(ENOENT)
    }

    /// Transfer cleanup to the caller without making the name available again.
    pub(super) fn remove(&mut self, id: u64) -> Result<T> {
        if id == 0 {
            return Err(EINVAL);
        }
        let index = self
            .entries
            .iter()
            .position(|entry| entry.id == id)
            .ok_or(ENOENT)?;
        Ok(self.entries.remove(index).map_err(|_| EIO)?.value)
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
