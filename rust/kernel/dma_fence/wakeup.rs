// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Cancellable completion notifications without retaining native work ownership.

use super::Fence;
use crate::{
    error::from_err_ptr,
    prelude::*,
    sync::{
        aref::ARef,
        poll::PollCondVar,
        Arc, //
    }, //
};
use core::ptr::NonNull;

/// Wake a retained condition variable when a concrete native fence signals.
///
/// Notification is advisory; register a waiter before checking authoritative state.
/// Dropping this object detaches notification without waiting for the fence or ending
/// native access. Drop requires sleepable context outside the fence's signaling lock.
/// No callback into the creating module survives destruction of this observation.
pub struct Wakeup {
    raw: NonNull<bindings::dma_fence_wakeup>,
    fence: ARef<Fence>,
    _changed: Arc<PollCondVar>,
}

// SAFETY: The native observation retains its fence and serializes detachment with signaling.
// Its stable waitqueue is retained until Drop has finished detaching the callback.
unsafe impl Send for Wakeup {}
// SAFETY: Shared access only returns the retained fence. Destruction requires exclusive ownership.
unsafe impl Sync for Wakeup {}

impl Wakeup {
    /// Allocate notification ownership. An already completed fence immediately wakes waiters.
    pub fn new(fence: &Fence, changed: Arc<PollCondVar>) -> Result<Self> {
        // SAFETY: Both inputs remain live and pinned. On success the returned wrapper retains
        // the waitqueue until native destruction has detached or waited out its callback.
        let raw = from_err_ptr(unsafe {
            bindings::dma_fence_wakeup_create(fence.as_raw(), changed.wait_queue_head.get())
        })?;
        Ok(Self {
            // SAFETY: Successful native construction returns a non-null owned observation.
            raw: unsafe { NonNull::new_unchecked(raw) },
            fence: fence.to_owned_ref(),
            _changed: changed,
        })
    }

    /// The observed native identity and status, independently of notification delivery.
    pub fn fence(&self) -> &Fence {
        &self.fence
    }
}

impl Drop for Wakeup {
    fn drop(&mut self) {
        // SAFETY: This is the unique native observation owner. The retained waitqueue and
        // fence remain live throughout callback detachment and native record destruction.
        unsafe { bindings::dma_fence_wakeup_destroy(self.raw.as_ptr()) };
    }
}

#[cfg(CONFIG_KUNIT)]
#[kunit_tests(rust_dma_fence_wakeup)]
mod tests {
    use super::*;
    use crate::dma_fence::{testing::ManualFence, Status};

    #[test]
    fn detachment_does_not_complete_native_work() -> Result {
        let changed = Arc::pin_init(crate::new_poll_condvar!(), GFP_KERNEL)?;
        let mut completion = ManualFence::new()?;
        let fence = completion.fence();
        let wakeup = Wakeup::new(&fence, changed)?;
        assert!(core::ptr::eq(wakeup.fence(), &*fence));
        drop(wakeup);
        assert_eq!(fence.status(), Status::Pending);
        completion.complete(Ok(()))?;
        assert_eq!(fence.status(), Status::Complete(Ok(())));
        Ok(())
    }

    #[test]
    fn observation_retains_the_waitqueue_and_completed_status() -> Result {
        for result in [Ok(()), Err(EAGAIN)] {
            let changed = Arc::pin_init(crate::new_poll_condvar!(), GFP_KERNEL)?;
            let mut completion = ManualFence::new()?;
            let wakeup = Wakeup::new(&completion.fence(), changed)?;
            completion.complete(result)?;
            drop(completion);
            assert_eq!(wakeup.fence().status(), Status::Complete(result));
            drop(wakeup);
        }
        Ok(())
    }

    #[test]
    fn completed_fences_remain_observable_after_registration() -> Result {
        let changed = Arc::pin_init(crate::new_poll_condvar!(), GFP_KERNEL)?;
        let mut completion = ManualFence::new()?;
        completion.complete(Err(EIO))?;
        let wakeup = Wakeup::new(&completion.fence(), changed)?;
        assert_eq!(wakeup.fence().status(), Status::Complete(Err(EIO)));
        Ok(())
    }
}
