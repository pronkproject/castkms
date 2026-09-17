// SPDX-License-Identifier: GPL-2.0-only

//! Exact accepted backend ownership through a source job's native completion.

use kernel::{
    dma_fence::{
        retirement::{Retire, Retirement},
        Fence,
    },
    drm::constraints::OpaqueEntry,
    prelude::*,
    sync::aref::ARef,
};

struct Hold(ARef<OpaqueEntry>);

// SAFETY: The module retains the callback; the native entry retains its backend module.
#[vtable]
unsafe impl Retire for Hold {
    fn retire(self) {
        drop(self.0);
    }
}

/// A job reserves cleanup before it can publish source access to the worker.
/// Job admission bounds unresolved owners; abandonment is not native completion.
pub(super) struct Retained(Option<Retirement<Hold>>);

impl Retained {
    pub(super) fn new(entry: Option<&OpaqueEntry>) -> Result<Self> {
        Ok(Self(
            entry
                .map(|entry| Retirement::new(Hold(ARef::from(entry))))
                .transpose()?,
        ))
    }

    /// No further access can be submitted. CPU/no-access releases need no native wait.
    pub(super) fn finish(mut self, fence: Option<&Fence>) {
        if let Some(retirement) = self.0.take() {
            if let Some(fence) = fence {
                retirement.submit(fence);
            }
        }
    }
}

impl Drop for Retained {
    fn drop(&mut self) {
        if let Some(retirement) = self.0.take() {
            // Missing completion permanently quarantines this admitted backend ownership.
            // Neither file closure nor selecting another entry proves native work ended.
            core::mem::forget(retirement);
        }
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_source_backend)]
mod tests {
    use super::*;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use kernel::{
        dma_fence::testing::ManualFence,
        drm::{
            constraints::{Backend, Description, Domain, Entry, Format, Size},
            fourcc,
        },
        sync::Arc,
        time::{delay::fsleep, Delta, Instant, Monotonic},
    };

    struct Resource(Arc<AtomicUsize>);

    // SAFETY: The local module owns the destructor and its atomic bookkeeping.
    #[vtable]
    unsafe impl Backend for Resource {}

    impl Drop for Resource {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Release);
        }
    }

    fn entry(retired: &Arc<AtomicUsize>) -> Result<ARef<Entry<Resource>>> {
        let domain = Domain::new(1)?;
        let size = Size::exact(640, 480);
        let description =
            Description::new(size, &[Format::implicit(7, fourcc::XRGB8888, size)], &[])?;
        Entry::new(
            &domain,
            3,
            &description,
            Arc::new(Resource(retired.clone()), GFP_KERNEL)?,
        )
    }

    #[test]
    fn scene_clones_preserve_exact_backend_identity() -> Result {
        let retired = Arc::new(AtomicUsize::new(0), GFP_KERNEL)?;
        let first = entry(&retired)?;
        let second = entry(&retired)?;
        assert_eq!(first.id(), second.id());
        let mut scene = crate::scene::Scene::blank(None);
        scene.set_constraints(Some(&first));
        let old = scene.clone();
        scene.set_constraints(Some(&second));
        assert!(core::ptr::eq(old.constraints().unwrap(), &**first));
        assert!(core::ptr::eq(scene.constraints().unwrap(), &**second));
        drop(first);
        drop(second);
        assert_eq!(retired.load(Ordering::Acquire), 0);
        scene.set_constraints(None);
        assert_eq!(retired.load(Ordering::Acquire), 1);
        drop(old);
        assert_eq!(retired.load(Ordering::Acquire), 2);
        Ok(())
    }

    #[test]
    fn submitted_backend_survives_scene_release_until_native_completion() -> Result {
        for result in [Ok(()), Err(EIO)] {
            let retired = Arc::new(AtomicUsize::new(0), GFP_KERNEL)?;
            let entry = entry(&retired)?;
            let mut scene = crate::scene::Scene::blank(None);
            scene.set_constraints(Some(&entry));
            let retained = Retained::new(scene.constraints())?;
            let mut fence = ManualFence::new()?;
            retained.finish(Some(&fence.fence()));
            drop(scene);
            drop(entry);
            assert_eq!(retired.load(Ordering::Acquire), 0);
            fence.complete(result)?;
            let start = Instant::<Monotonic>::now();
            while retired.load(Ordering::Acquire) == 0 {
                if start.elapsed() > Delta::from_secs(1) {
                    return Err(ETIMEDOUT);
                }
                fsleep(Delta::from_millis(1));
            }
            assert_eq!(retired.load(Ordering::Acquire), 1);
        }
        Ok(())
    }

    #[test]
    fn synchronous_release_does_not_keep_backend_resources() -> Result {
        let retired = Arc::new(AtomicUsize::new(0), GFP_KERNEL)?;
        let entry = entry(&retired)?;
        let retained = Retained::new(Some(&entry))?;
        drop(entry);
        assert_eq!(retired.load(Ordering::Acquire), 0);
        retained.finish(None);
        assert_eq!(retired.load(Ordering::Acquire), 1);
        Retained::new(None)?.finish(None);
        Ok(())
    }
}
