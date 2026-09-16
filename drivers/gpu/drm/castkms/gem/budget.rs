// SPDX-License-Identifier: GPL-2.0-only

//! Storage reservations independent of handles, mappings, and export lifetimes.

use kernel::{
    prelude::*,
    sync::{
        Arc,
        Mutex, //
    }, //
};

/// A caller-owned limit shared by current and retired allocations.
#[pin_data]
pub(crate) struct Budget {
    limit: usize,
    #[pin]
    used: Mutex<usize>,
}

impl Budget {
    pub(crate) fn new(limit: usize) -> Result<Arc<Self>> {
        if limit == 0 {
            return Err(EINVAL);
        }
        Arc::pin_init(
            pin_init!(Self {
                limit,
                used <- kernel::new_mutex!(0),
            }),
            GFP_KERNEL,
        )
    }

    /// Reserve storage without waiting for another allocation to disappear.
    pub(super) fn reserve(self: &Arc<Self>, bytes: usize) -> Result<Charge> {
        if bytes == 0 {
            return Err(EINVAL);
        }
        if bytes > self.limit {
            return Err(E2BIG);
        }
        let mut used = self.used.lock();
        if bytes > self.limit - *used {
            return Err(EBUSY);
        }
        *used += bytes;
        Ok(Charge {
            budget: self.clone(),
            bytes,
        })
    }
}

/// Unique credit returned only when the owning allocation releases its payload.
pub(super) struct Charge {
    budget: Arc<Budget>,
    pub(super) bytes: usize,
}

impl Drop for Charge {
    fn drop(&mut self) {
        *self.budget.used.lock() -= self.bytes;
    }
}
