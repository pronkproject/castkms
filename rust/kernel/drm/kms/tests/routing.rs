// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Connector routes observed through the borrowed atomic state.

use super::*;
use connector::{
    AsRawConnector,
    RawConnector, //
};
use crtc::{
    AsRawCrtc,
    RawCrtcState, //
};

#[kunit_tests(rust_drm_routing)]
mod tests {
    use super::*;

    #[test]
    fn connector_mask_follows_transaction_routes() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-routes", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        // SAFETY: The private initialized device retains its objects through both checks.
        let crtc = unsafe { crtc::Crtc::<TestCrtc>::from_raw(dev.crtc.load(Ordering::Relaxed)) };
        let connector = unsafe {
            connector::Connector::<TestConnector>::from_raw(dev.connector.load(Ordering::Relaxed))
        };
        let fb = framebuffer(&dev)?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[connector],
            position: (0, 0),
        };
        let update = |mut state: Pin<&mut atomic::AtomicStateComposer<TestDriver>>| {
            if state.add_crtc_state(crtc)?.connector_mask() != 0 {
                return Err(EINVAL);
            }
            state.as_mut().set_crtc_config(crtc, Some(&scanout))?;
            if state.add_crtc_state(crtc)?.connector_mask() != connector.mask() {
                return Err(EINVAL);
            }
            state.as_mut().set_crtc_config(crtc, None)?;
            if state.add_crtc_state(crtc)?.connector_mask() != 0 {
                return Err(EINVAL);
            }
            state.as_mut().set_crtc_config(crtc, Some(&scanout))?;
            Err(ECANCELED)
        };
        // SAFETY: No registration or teardown overlaps these canceled private transactions.
        if unsafe { atomic::run_check(&dev, update) } != Err(ECANCELED) {
            return Err(EINVAL);
        }
        let unchanged = |state: Pin<&mut atomic::AtomicStateComposer<TestDriver>>| {
            if state.add_crtc_state(crtc)?.connector_mask() != 0 {
                return Err(EINVAL);
            }
            Err(ECANCELED)
        };
        // SAFETY: The same exclusively owned configuration remains alive until return.
        if unsafe { atomic::run_check(&dev, unchanged) } != Err(ECANCELED) {
            return Err(EINVAL);
        }
        Ok(())
    }
}
