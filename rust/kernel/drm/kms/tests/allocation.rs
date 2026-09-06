// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Task-local slab failure for private test payloads in disposable kernels.

use crate::{
    alloc::flags::{__GFP_NOWARN, GFP_NOWAIT},
    prelude::*,
    sync::atomic::{Atomic, Relaxed},
    task::Task,
};

pub(super) fn fail_value_allocation(value: u64) -> Result<(Result<KBox<u64>>, bool)> {
    let task = Task::current_raw();
    // SAFETY: The current task remains alive throughout this synchronous allocation.
    // fail_nth is an initialized, aligned u32, accessed with READ_ONCE/WRITE_ONCE in C.
    let counter = unsafe { Atomic::<u32>::from_ptr(&raw mut (*task).fail_nth) };
    // Disposable-kernel tests must not run alongside external fault configuration.
    // Refuse to replace an existing task-local injection request.
    if counter.load(Relaxed) != 0 {
        return Err(EBUSY);
    }
    // No allocation, assertion or early return intervenes between setting the counter
    // and the tested allocation.
    counter.store(1, Relaxed);
    let result = KBox::new(value, GFP_NOWAIT | __GFP_NOWARN).map_err(Error::from);
    // The allocation has returned. Clear any unconsumed request before returning
    // to callback error handling, so teardown allocations cannot inherit the failure.
    let remaining = counter.load(Relaxed);
    counter.store(0, Relaxed);
    Ok((result, remaining == 0))
}
