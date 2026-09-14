// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Reservation observations through retained native DMA-BUF ownership.

use super::*;
use crate::{
    dma_fence::{
        testing::ManualFence,
        Status, //
    },
    dma_resv::Usage,
    drm::kms::testing::TestDevice, //
};

#[kunit_tests(rust_drm_dma_buf_reservations)]
mod cases {
    use super::*;

    #[test]
    fn aliases_borrow_one_native_reservation() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-buffer-reservation", None)?;
        let result = (|| {
            let fixture = TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
            let buffer = fixture.export_dumb(64, 64, 32)?;
            let alias = buffer.clone();
            if !core::ptr::eq(buffer.reservation(), alias.reservation())
                || !buffer.reservation().snapshot(Usage::Read)?.is_empty()
            {
                return Err(EINVAL);
            }
            drop(fixture);
            drop(buffer);
            if !alias.reservation().snapshot(Usage::Read)?.is_empty() {
                return Err(EINVAL);
            }
            Ok(())
        })();
        // SAFETY: Private exports are gone; drain final native file release with no locks
        // held and while the faux parent is still bound.
        unsafe { bindings::flush_delayed_fput() };
        result
    }

    #[test]
    fn acquired_records_outlive_the_export() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-buffer-record", None)?;
        let result = (|| {
            let fixture = TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
            let buffer = fixture.export_dumb(64, 64, 32)?;
            let mut producer = ManualFence::new()?;
            let fence = producer.fence();
            // SAFETY: The fixture owns this private export and no other task submits work.
            let raw = unsafe { (*buffer.as_raw()).resv };
            // SAFETY: Lock one live reservation without an enclosing acquisition context.
            crate::error::to_result(unsafe {
                bindings::dma_resv_lock(raw, core::ptr::null_mut())
            })?;
            // SAFETY: Native insertion requires a reserved entry under the same update lock.
            let reserved =
                crate::error::to_result(unsafe { bindings::dma_resv_reserve_fences(raw, 1) });
            if reserved.is_ok() {
                // SAFETY: The slot was reserved, the lock remains held, and insertion takes
                // its own native reference to this manually controlled test fence.
                unsafe { bindings::dma_resv_add_fence(raw, fence.as_raw(), Usage::Read as _) };
            }
            // SAFETY: Balance the successful lock regardless of allocation outcome.
            unsafe { bindings::dma_resv_unlock(raw) };
            reserved?;
            let snapshot = buffer.reservation().snapshot(Usage::Read)?;
            drop(buffer);
            drop(fixture);
            producer.complete(Err(EIO))?;
            if snapshot.len() != 1
                || snapshot.iter().next().map(|record| record.status())
                    != Some(Status::Complete(Err(EIO)))
            {
                return Err(EINVAL);
            }
            Ok(())
        })();
        // SAFETY: Drain private export release before its faux parent, with no locks held.
        unsafe { bindings::flush_delayed_fput() };
        result
    }
}
