// SPDX-License-Identifier: GPL-2.0-only

//! Private image storage charged until the final allocation is released.

use kernel::{
    prelude::*,
    sync::{
        Arc,
        Mutex, //
    }, //
};

pub(crate) const LIMIT: usize = 512 * 1024 * 1024;

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;

/// One output's private image budget, shared by current and retired pools.
#[pin_data]
pub(crate) struct Budget {
    #[pin]
    used: Mutex<usize>,
}

impl Budget {
    pub(crate) fn new() -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                used <- kernel::new_mutex!(0),
            }),
            GFP_KERNEL,
        )
    }

    /// Reserve bytes without waiting for an older allocation to be released.
    pub(crate) fn reserve(self: &Arc<Self>, bytes: usize) -> Result<Charge> {
        if bytes == 0 {
            return Err(EINVAL);
        }
        if bytes > LIMIT {
            return Err(E2BIG);
        }
        let mut used = self.used.lock();
        if bytes > LIMIT - *used {
            return Err(EBUSY);
        }
        *used += bytes;
        Ok(Charge {
            budget: self.clone(),
            bytes,
        })
    }
}

/// Unique ownership of reserved bytes; keep it until the allocation is destroyed.
pub(crate) struct Charge {
    budget: Arc<Budget>,
    bytes: usize,
}

impl Drop for Charge {
    fn drop(&mut self) {
        *self.budget.used.lock() -= self.bytes;
    }
}
