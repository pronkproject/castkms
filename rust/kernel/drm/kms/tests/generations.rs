// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Accounting identities retained by accepted CRTC callbacks, without pixel access.

use super::*;
use crate::{
    drm::preparation::Source,
    sync::aref::ARef, //
};
use crtc::AsRawCrtc;

/// The slot owns one native reference. Replacing or taking it transfers that ownership.
#[derive(Default)]
pub(super) struct Observed {
    source: AtomicPtr<Source>,
    calls: AtomicU32,
}

impl Observed {
    pub(super) fn record(&self, source: Option<ARef<Source>>) {
        let raw = source.map_or(ptr::null_mut(), |source| ARef::into_raw(source).as_ptr());
        let previous = self.source.swap(raw, Ordering::AcqRel);
        if let Some(previous) = NonNull::new(previous) {
            // SAFETY: The swap transfers the slot's sole owned reference to this invocation.
            drop(unsafe { ARef::from_raw(previous) });
        }
        self.calls.fetch_add(1, Ordering::Relaxed);
    }

    fn take(&self) -> Option<ARef<Source>> {
        let source = NonNull::new(self.source.swap(ptr::null_mut(), Ordering::AcqRel))?;
        // SAFETY: The swap removes and transfers the slot's owned reference exactly once.
        Some(unsafe { ARef::from_raw(source) })
    }
}

impl Drop for Observed {
    fn drop(&mut self) {
        drop(self.take());
    }
}

pub(super) fn expand_check(state: &atomic::AtomicStateComposer<TestDriver>) -> Result {
    let dev = state.drm_dev();
    if dev
        .counts
        .include_crtc_during_plane_check
        .load(Ordering::Relaxed)
        == 0
    {
        return Ok(());
    }
    // SAFETY: The callback retains the fixture's completed device. Setup installed the only
    // CRTC before transactions became possible, and teardown cannot overlap the callback.
    let crtc = unsafe { crtc::Crtc::<TestCrtc>::from_raw(dev.crtc.load(Ordering::Relaxed)) };
    drop(state.add_crtc_state(crtc)?);
    Ok(())
}

fn check_installed(dev: &testing::TestDevice<TestDriver>, source: &Source) -> Result {
    dev.check(|transaction| {
        let crtc = dev.crtc()?;
        let _state = transaction.add_crtc_state(crtc)?;
        // SAFETY: The state guard retains the CRTC lock. The fixture retains the device and
        // the previously accepted state while the native accessor returns its borrowed source.
        let installed = unsafe { bindings::drm_atomic_prepare_crtc_source(crtc.as_raw()) };
        if installed.cast_const() != ptr::from_ref(source).cast() {
            return Err(EINVAL);
        }
        Ok(())
    })
}

#[kunit_tests(rust_drm_accepted_generations)]
mod cases {
    use super::*;

    #[test]
    fn devices_without_accounting_report_no_generation() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-generation-disabled", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let fb = framebuffer(dev.device())?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        dev.update(|transaction| transaction.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        assert_eq!(counts.accepted_generation.calls.load(Ordering::Relaxed), 1);
        assert!(counts.accepted_generation.take().is_none());
        Ok(())
    }

    #[test]
    fn accepted_generation_survives_device_teardown() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.preparation_capacity.store(2, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-generation-retained", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let fb = framebuffer(dev.device())?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        dev.update(|transaction| transaction.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        let source = counts.accepted_generation.take().ok_or(EINVAL)?;
        check_installed(&dev, &source)?;
        drop(fb);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert!(source.hold_admission()?.prepared()?.is_some());
        Ok(())
    }

    #[test]
    fn same_framebuffer_replacement_has_a_new_generation() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.preparation_capacity.store(2, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-generation-recommit", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let fb = framebuffer(dev.device())?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        dev.update(|transaction| transaction.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        let first = counts.accepted_generation.take().ok_or(EINVAL)?;
        dev.update(|transaction| transaction.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        let second = counts.accepted_generation.take().ok_or(EINVAL)?;
        assert!(!ptr::eq(&*first, &*second));
        check_installed(&dev, &second)?;
        assert_eq!(counts.accepted_generation.calls.load(Ordering::Relaxed), 2);
        Ok(())
    }

    #[test]
    fn disabling_the_output_retains_its_accepted_generation() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.preparation_capacity.store(2, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-generation-blank", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let fb = framebuffer(dev.device())?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        dev.update(|transaction| transaction.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        let first = counts.accepted_generation.take().ok_or(EINVAL)?;
        dev.update(|transaction| transaction.set_crtc_config(dev.crtc()?, None))?;
        let disabled = counts.accepted_generation.take().ok_or(EINVAL)?;
        assert!(!ptr::eq(&*first, &*disabled));
        check_installed(&dev, &disabled)?;
        assert_eq!(counts.disables.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn validation_added_crtc_gets_the_accepted_generation() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.preparation_capacity.store(2, Ordering::Relaxed);
        counts
            .include_crtc_during_plane_check
            .store(1, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-generation-expanded", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        dev.update(|transaction| {
            drop(transaction.add_plane_state(dev.plane()?)?);
            if transaction.get_new_crtc_state(dev.crtc()?).is_some() {
                return Err(EINVAL);
            }
            Ok(())
        })?;
        let accepted = counts.accepted_generation.take().ok_or(EINVAL)?;
        check_installed(&dev, &accepted)?;
        assert_eq!(counts.accepted_generation.calls.load(Ordering::Relaxed), 1);
        assert_eq!(counts.enables.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn checks_and_rejected_commits_do_not_publish_generations() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.preparation_capacity.store(2, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-generation-rejected", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let fb = framebuffer(dev.device())?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        dev.update(|transaction| transaction.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        let accepted = counts.accepted_generation.take().ok_or(EINVAL)?;
        dev.check(|transaction| transaction.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        counts
            .fail_framebuffer_preparation
            .store(1, Ordering::Relaxed);
        assert_eq!(
            dev.update(|transaction| transaction.set_crtc_config(dev.crtc()?, Some(&scanout))),
            Err(ENOMEM)
        );
        counts
            .fail_framebuffer_preparation
            .store(0, Ordering::Relaxed);
        assert_eq!(counts.accepted_generation.calls.load(Ordering::Relaxed), 1);
        assert!(counts.accepted_generation.take().is_none());
        check_installed(&dev, &accepted)?;
        Ok(())
    }
}
