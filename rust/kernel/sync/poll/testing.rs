// SPDX-License-Identifier: GPL-2.0

//! Waitqueue notification observations for kernel tests.

use super::PollCondVar;
use crate::{bindings, prelude::*, sync::Arc, types::Opaque};
use core::sync::atomic::{AtomicUsize, Ordering};

struct Entry {
    native: Opaque<bindings::wait_queue_entry>,
    notifications: AtomicUsize,
}

/// Count actual native wake callbacks without sleeping or inspecting provider state.
///
/// The observer keeps its condition variable alive. Dropping it removes its waiter
/// synchronously before freeing the entry. The counter may wrap after `usize::MAX`.
pub struct Observer {
    entry: KBox<Entry>,
    changed: Arc<PollCondVar>,
}

impl Observer {
    /// Register before changing or inspecting the state whose notifications are tested.
    pub fn new(changed: Arc<PollCondVar>) -> Result<Self> {
        let entry = KBox::new(
            Entry {
                native: Opaque::zeroed(),
                notifications: AtomicUsize::new(0),
            },
            GFP_KERNEL,
        )?;
        let raw = entry.native.get();
        // SAFETY: The fresh entry is stable in its private allocation. These assignments
        // match init_waitqueue_func_entry; add_wait_queue initializes its list links.
        // The retained condition variable outlives removal in Observer::drop.
        unsafe {
            (*raw).func = Some(notified);
            (*raw).private = (&*entry as *const Entry).cast_mut().cast();
            bindings::add_wait_queue(changed.wait_queue_head.get(), raw);
        }
        Ok(Self { entry, changed })
    }

    /// Number of wake callbacks observed so far, not a count of state transitions.
    pub fn notifications(&self) -> usize {
        self.entry.notifications.load(Ordering::Relaxed)
    }
}

unsafe extern "C" fn notified(
    entry: *mut bindings::wait_queue_entry,
    _mode: u32,
    _flags: i32,
    _key: *mut core::ffi::c_void,
) -> i32 {
    // SAFETY: Only Observer::new installs this callback. Its private pointer names
    // stable retained storage; removal takes the same waitqueue lock as invocation.
    let entry = unsafe { &*(*entry).private.cast::<Entry>() };
    entry.notifications.fetch_add(1, Ordering::Relaxed);
    0
}

impl Drop for Observer {
    fn drop(&mut self) {
        // SAFETY: The owned entry is still registered on the retained condition variable.
        // Removal synchronizes with its callback before either allocation can be freed.
        unsafe {
            bindings::remove_wait_queue(self.changed.wait_queue_head.get(), self.entry.native.get())
        };
    }
}

#[kunit_tests(rust_poll_notifications)]
mod tests {
    use super::*;

    #[test]
    fn observers_receive_wakeups_and_detach_independently() -> Result {
        let changed = Arc::pin_init(crate::new_poll_condvar!(), GFP_KERNEL)?;
        let first = Observer::new(changed.clone())?;
        let second = Observer::new(changed.clone())?;
        assert_eq!(first.notifications(), 0);
        changed.notify_all();
        assert_eq!(first.notifications(), 1);
        assert_eq!(second.notifications(), 1);
        drop(first);
        changed.notify_all();
        assert_eq!(second.notifications(), 2);
        drop(changed);
        assert_eq!(second.notifications(), 2);
        Ok(())
    }
}
