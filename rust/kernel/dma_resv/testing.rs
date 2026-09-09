// SPDX-License-Identifier: GPL-2.0

//! Private reservation owners for kernel tests, without access to production reservations.

use super::*;

/// Own a stable reservation allocation with serialized test-only insertion.
pub struct TestReservation {
    storage: KBox<Opaque<bindings::dma_resv>>,
}

impl TestReservation {
    /// Initialize a private native reservation.
    pub fn new() -> Result<Self> {
        let storage = KBox::new(Opaque::<bindings::dma_resv>::zeroed(), GFP_KERNEL)?;
        // SAFETY: Fresh stable allocation with no references or initialized native fields.
        unsafe { bindings::dma_resv_init(storage.get()) };
        Ok(Self { storage })
    }

    /// Borrow read-only access for the fixture owner's lifetime.
    pub fn reservation(&self) -> &Reservation {
        // SAFETY: The owner retains the initialized stable allocation until after the borrow.
        unsafe { Reservation::from_raw(self.storage.get()) }
    }

    /// Add a retained test fence with the requested native usage.
    pub fn add(&mut self, fence: &Fence, usage: Usage) -> Result {
        let raw = self.storage.get();
        // SAFETY: A single private reservation is locked with no enclosing acquire context.
        to_result(unsafe { bindings::dma_resv_lock(raw, core::ptr::null_mut()) })?;
        // SAFETY: The update lock is held. Native insertion acquires its own fence reference
        // and requires a successful slot reservation before modifying the list.
        let result = to_result(unsafe { bindings::dma_resv_reserve_fences(raw, 1) });
        if result.is_ok() {
            unsafe { bindings::dma_resv_add_fence(raw, fence.as_raw(), usage as _) };
        }
        // SAFETY: Balance the successful lock on both allocation outcomes.
        unsafe { bindings::dma_resv_unlock(raw) };
        result
    }
}

impl Drop for TestReservation {
    fn drop(&mut self) {
        // SAFETY: No reservation borrows remain. Native finalization drops its fence entries
        // before the backing allocation is released by KBox.
        unsafe { bindings::dma_resv_fini(self.storage.get()) };
    }
}

#[kunit_tests(rust_dma_resv)]
mod cases {
    use super::*;
    use crate::dma_fence::{
        testing::ManualFence,
        Status, //
    };

    #[test]
    fn empty_snapshot_owns_no_records() -> Result {
        let owner = TestReservation::new()?;
        let snapshot = owner.reservation().snapshot(Usage::Bookkeep)?;
        drop(owner);
        assert!(snapshot.is_empty());
        assert_eq!(snapshot.iter().count(), 0);
        Ok(())
    }

    #[test]
    fn acquired_failure_survives_reservation_destruction() -> Result {
        let mut owner = TestReservation::new()?;
        let mut producer = ManualFence::new()?;
        owner.add(&producer.fence(), Usage::Write)?;
        let snapshot = owner.reservation().snapshot(Usage::Write)?;
        assert_eq!(snapshot.len(), 1);
        drop(owner);
        producer.complete(Err(EIO))?;
        drop(producer);
        assert_eq!(
            snapshot.iter().next().unwrap().status(),
            Status::Complete(Err(EIO))
        );
        Ok(())
    }

    #[test]
    fn completed_errors_are_not_reservation_history() -> Result {
        let mut owner = TestReservation::new()?;
        let mut producer = ManualFence::new()?;
        owner.add(&producer.fence(), Usage::Write)?;
        let before = owner.reservation().snapshot(Usage::Write)?;
        producer.complete(Err(EIO))?;
        let after = owner.reservation().snapshot(Usage::Write)?;
        assert_eq!(before.len(), 1);
        assert_eq!(
            before.iter().next().unwrap().status(),
            Status::Complete(Err(EIO))
        );
        assert!(after.is_empty());
        Ok(())
    }

    #[test]
    fn usage_selects_dependencies_without_bookkeeping_leakage() -> Result {
        let mut owner = TestReservation::new()?;
        let kernel = ManualFence::new()?;
        let writer = ManualFence::new()?;
        let reader = ManualFence::new()?;
        let bookkeeping = ManualFence::new()?;
        owner.add(&kernel.fence(), Usage::Kernel)?;
        owner.add(&writer.fence(), Usage::Write)?;
        owner.add(&reader.fence(), Usage::Read)?;
        owner.add(&bookkeeping.fence(), Usage::Bookkeep)?;
        assert_eq!(owner.reservation().snapshot(Usage::Kernel)?.len(), 1);
        assert_eq!(owner.reservation().snapshot(Usage::Write)?.len(), 2);
        assert_eq!(owner.reservation().snapshot(Usage::Read)?.len(), 3);
        assert_eq!(owner.reservation().snapshot(Usage::Bookkeep)?.len(), 4);
        Ok(())
    }

    #[test]
    fn snapshot_excludes_later_insertions() -> Result {
        let mut owner = TestReservation::new()?;
        let first = ManualFence::new()?;
        let second = ManualFence::new()?;
        owner.add(&first.fence(), Usage::Write)?;
        let before = owner.reservation().snapshot(Usage::Write)?;
        owner.add(&second.fence(), Usage::Write)?;
        assert_eq!(before.len(), 1);
        assert_eq!(owner.reservation().snapshot(Usage::Write)?.len(), 2);
        Ok(())
    }
}
