// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::drm::kms::crtc::RawCrtcState;

#[kunit_tests(rust_drm_atomic_installation)]
mod cases {
    use super::*;

    #[test]
    fn installation_rejection_preserves_state_and_allows_retry() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-install-rejection", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let fb = framebuffer(dev.device())?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        counts.fail_install.store(1, Ordering::Relaxed);
        dev.check(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        assert_eq!(counts.install_calls.load(Ordering::Relaxed), 0);
        assert_eq!(
            dev.update(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout))),
            Err(EAGAIN)
        );
        assert_eq!(counts.install_calls.load(Ordering::Relaxed), 1);
        assert_eq!(counts.install_successes.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_updates.load(Ordering::Relaxed), 0);
        dev.check(|transaction| {
            let state = transaction.add_crtc_state(dev.crtc()?)?;
            if state.active() {
                return Err(EINVAL);
            }
            Ok(())
        })?;
        counts.fail_install.store(0, Ordering::Relaxed);
        dev.update(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        assert_eq!(counts.install_successes.load(Ordering::Relaxed), 1);
        assert_eq!(counts.plane_updates.load(Ordering::Relaxed), 1);
        drop(fb);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
