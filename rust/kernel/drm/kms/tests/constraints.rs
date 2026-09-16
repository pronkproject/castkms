// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;

#[kunit_tests(rust_drm_kms_constraints)]
mod cases {
    use super::*;

    #[test]
    fn final_provider_recheck_rejects_before_state_swap() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.constraints_capacity.store(4, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-kms-constraints-recheck", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let fb = framebuffer(dev.device())?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        counts.fail_constraints_at_install.store(1, Ordering::Relaxed);
        assert_eq!(
            dev.update(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout))),
            Err(EIO)
        );
        assert_eq!(counts.constraints_checks.load(Ordering::Relaxed), 2);
        assert_eq!(counts.install_calls.load(Ordering::Relaxed), 1);
        assert_eq!(counts.install_successes.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_updates.load(Ordering::Relaxed), 0);
        counts.fail_constraints_at_install.store(0, Ordering::Relaxed);
        counts.fail_constraints.store(0, Ordering::Relaxed);
        dev.update(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        assert_eq!(counts.constraints_checks.load(Ordering::Relaxed), 4);
        assert_eq!(counts.install_successes.load(Ordering::Relaxed), 1);
        drop(fb);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn attached_default_checks_complete_proposal_before_installation() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.constraints_capacity.store(4, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-kms-constraints", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        assert_ne!(counts.constraints_id.load(Ordering::Relaxed), 0);
        let fb = framebuffer(dev.device())?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        dev.check(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        assert_eq!(counts.constraints_checks.load(Ordering::Relaxed), 1);
        assert_eq!(counts.install_successes.load(Ordering::Relaxed), 0);
        counts.fail_constraints.store(1, Ordering::Relaxed);
        assert_eq!(
            dev.update(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout))),
            Err(EIO)
        );
        assert_eq!(counts.install_successes.load(Ordering::Relaxed), 0);
        counts.fail_constraints.store(0, Ordering::Relaxed);
        dev.update(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        // One successful check, one rejected check, then validation and final acceptance.
        assert_eq!(counts.constraints_checks.load(Ordering::Relaxed), 4);
        assert_eq!(counts.install_successes.load(Ordering::Relaxed), 1);
        assert_eq!(counts.plane_updates.load(Ordering::Relaxed), 1);
        // Provider failure must not prevent shutdown of the accepted default.
        counts.fail_constraints.store(1, Ordering::Relaxed);
        drop(fb);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
