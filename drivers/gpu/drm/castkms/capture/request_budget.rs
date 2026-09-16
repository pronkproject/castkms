// SPDX-License-Identifier: GPL-2.0-only

//! Bounded delegated capture bookkeeping, independent of image storage budgets.

use kernel::{
    prelude::*,
    sync::{
        Arc,
        Mutex, //
    }, //
};

pub(crate) const CAPACITY_LIMIT: u32 = 8;
const QUEUE_LIMIT: usize = 16;

#[pin_data]
pub(crate) struct Budget {
    #[pin]
    queues: Mutex<usize>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Budget {
    pub(crate) fn new() -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self { queues <- kernel::new_mutex!(0) }),
            GFP_KERNEL,
        )
    }

    /// Reserve all records before admitting a stream. Native claims retain the charge
    /// independently of queue destruction, including when their completion is unresolved.
    pub(crate) fn reserve(self: &Arc<Self>, capacity: u32) -> Result<Arc<Charge>> {
        if capacity == 0 {
            return Err(EINVAL);
        }
        if capacity > CAPACITY_LIMIT {
            return Err(E2BIG);
        }
        {
            let mut queues = self.queues.lock();
            if *queues == QUEUE_LIMIT {
                return Err(EBUSY);
            }
            *queues += 1;
        }
        Ok(Arc::new(
            Charge {
                budget: self.clone(),
                capacity,
            },
            GFP_KERNEL,
        )?)
    }
}

/// One queue's record capacity, retained until its last native request owner retires.
pub(crate) struct Charge {
    budget: Arc<Budget>,
    capacity: u32,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Charge {
    pub(crate) fn capacity(&self) -> usize {
        self.capacity as usize
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        *self.budget.queues.lock() -= 1;
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_capture_request_budget)]
mod tests {
    use super::*;

    fn check(condition: bool) -> Result {
        if condition {
            Ok(())
        } else {
            Err(EINVAL)
        }
    }

    #[test]
    fn capacity_is_bounded_before_accounting() -> Result {
        let budget = Budget::new()?;
        check(budget.reserve(0).err() == Some(EINVAL))?;
        check(budget.reserve(CAPACITY_LIMIT + 1).err() == Some(E2BIG))?;
        let mut queues = KVec::new();
        for _ in 0..QUEUE_LIMIT {
            let charge = budget.reserve(CAPACITY_LIMIT)?;
            check(charge.capacity() == CAPACITY_LIMIT as usize)?;
            queues.push(charge, GFP_KERNEL)?;
        }
        check(budget.reserve(1).err() == Some(EBUSY))?;
        drop(queues);
        drop(budget.reserve(1)?);
        Ok(())
    }

    #[test]
    fn retained_native_owners_keep_queue_capacity_charged() -> Result {
        let budget = Budget::new()?;
        let mut queues = KVec::new();
        for _ in 0..QUEUE_LIMIT {
            queues.push(budget.reserve(1)?, GFP_KERNEL)?;
        }
        let native = queues[0].clone();
        drop(queues.remove(0)?);
        check(budget.reserve(1).err() == Some(EBUSY))?;
        drop(native);
        let replacement = budget.reserve(CAPACITY_LIMIT)?;
        check(budget.reserve(1).err() == Some(EBUSY))?;
        drop(replacement);
        Ok(())
    }
}
