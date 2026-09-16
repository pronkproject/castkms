// SPDX-License-Identifier: GPL-2.0-only

//! Bounded retained destination dependencies, including their native error history.

use kernel::{
    dma_fence::{wakeup::Wakeup, Fence, Status},
    dma_resv::Snapshot,
    prelude::*,
    sync::{aref::ARef, poll::PollCondVar, Arc},
};

const MAX_DEPENDENCIES: usize = 256;

struct Dependency {
    fence: ARef<Fence>,
    _wakeup: Option<Wakeup>,
}

/// Observations never authorize access or prevent another native submission.
/// Owners must exclude external submission before admitting a destination use.
/// Drop detaches all notifications, including fences which have not signaled.
pub(super) struct Dependencies {
    entries: KVec<Dependency>,
    changed: Option<Arc<PollCondVar>>,
}

impl Dependencies {
    pub(super) fn new(
        reuse: Option<&Fence>,
        snapshot: &Snapshot,
        changed: Option<Arc<PollCondVar>>,
    ) -> Result<Self> {
        let mut dependencies = Self {
            entries: KVec::new(),
            changed,
        };
        if let Some(reuse) = reuse {
            dependencies.retain(reuse)?;
        }
        dependencies.observe(snapshot)?;
        Ok(dependencies)
    }

    fn retain(&mut self, fence: &Fence) -> Result {
        if self
            .entries
            .iter()
            .any(|entry| core::ptr::eq(&*entry.fence, fence))
        {
            return Ok(());
        }
        if self.entries.len() == MAX_DEPENDENCIES {
            return Err(E2BIG);
        }
        let wakeup = self
            .changed
            .as_ref()
            .map(|changed| Wakeup::new(fence, changed.clone()))
            .transpose()?;
        self.entries.push(
            Dependency {
                fence: fence.to_owned_ref(),
                _wakeup: wakeup,
            },
            GFP_KERNEL,
        )?;
        Ok(())
    }

    pub(super) fn observe(&mut self, snapshot: &Snapshot) -> Result {
        for fence in snapshot.iter() {
            self.retain(fence)?;
        }
        Ok(())
    }

    pub(super) fn ready(&self) -> Result<bool> {
        let mut pending = false;
        for entry in &self.entries {
            match entry.fence.status() {
                Status::Pending => pending = true,
                Status::Complete(result) => result?,
            }
        }
        Ok(!pending)
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_reuse_dependencies)]
mod tests {
    use super::*;
    use kernel::{
        dma_fence::testing::ManualFence,
        dma_resv::{testing::TestReservation, Usage},
    };

    #[test]
    fn repeated_observation_preserves_error_without_spending_capacity() -> Result {
        let changed = Arc::pin_init(kernel::new_poll_condvar!(), GFP_KERNEL)?;
        let mut reservation = TestReservation::new()?;
        let mut producer = ManualFence::new()?;
        reservation.add(&producer.fence(), Usage::Read)?;
        let snapshot = reservation.reservation().snapshot(Usage::Read)?;
        let mut dependencies =
            Dependencies::new(Some(&producer.fence()), &snapshot, Some(changed))?;
        for _ in 0..MAX_DEPENDENCIES + 1 {
            dependencies.observe(&snapshot)?;
        }
        assert_eq!(dependencies.entries.len(), 1);
        assert_eq!(dependencies.ready(), Ok(false));
        producer.complete(Err(EAGAIN))?;
        let empty = reservation.reservation().snapshot(Usage::Read)?;
        assert!(empty.is_empty());
        dependencies.observe(&empty)?;
        assert_eq!(dependencies.ready(), Err(EAGAIN));
        Ok(())
    }

    #[test]
    fn late_observation_retains_failure_after_native_removal() -> Result {
        let mut reservation = TestReservation::new()?;
        let mut dependencies = Dependencies::new(
            None,
            &reservation.reservation().snapshot(Usage::Read)?,
            None,
        )?;
        assert_eq!(dependencies.ready(), Ok(true));
        let mut producer = ManualFence::new()?;
        reservation.add(&producer.fence(), Usage::Read)?;
        dependencies.observe(&reservation.reservation().snapshot(Usage::Read)?)?;
        assert_eq!(dependencies.ready(), Ok(false));
        producer.complete(Err(EIO))?;
        dependencies.observe(&reservation.reservation().snapshot(Usage::Read)?)?;
        assert_eq!(dependencies.ready(), Err(EIO));
        Ok(())
    }

    #[test]
    fn bounded_observations_detach_without_completing_native_work() -> Result {
        let changed = Arc::pin_init(kernel::new_poll_condvar!(), GFP_KERNEL)?;
        let reservation = TestReservation::new()?;
        let mut dependencies = Dependencies::new(
            None,
            &reservation.reservation().snapshot(Usage::Read)?,
            Some(changed),
        )?;
        let mut producers = KVec::new();
        for _ in 0..MAX_DEPENDENCIES {
            let producer = ManualFence::new()?;
            dependencies.retain(&producer.fence())?;
            producers.push(producer, GFP_KERNEL)?;
        }
        let extra = ManualFence::new()?;
        assert_eq!(dependencies.retain(&extra.fence()), Err(E2BIG));
        drop(dependencies);
        for mut producer in producers {
            assert_eq!(producer.fence().status(), Status::Pending);
            producer.complete(Ok(()))?;
        }
        Ok(())
    }
}
