// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use connector::AsRawConnector;
use crtc::AsRawCrtc;

#[kunit_tests(rust_drm_inspection)]
mod tests {
    use super::*;

    #[test]
    fn crtc_check_inspection_excludes_mutation() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-inspect-crtc", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        // SAFETY: The initialized device owns the observed CRTC throughout the test.
        let crtc = unsafe { crtc::Crtc::<TestCrtc>::from_raw(dev.crtc.load(Ordering::Relaxed)) };
        let update = |state: Pin<&mut atomic::AtomicStateComposer<TestDriver>>| {
            let guard = state.add_crtc_state(crtc)?;
            assert_eq!(
                state.try_for_each_new_crtc_state(|_, _| panic!("borrowed")),
                Err(EBUSY)
            );
            drop(guard);
            let mut visited = 0;
            state.try_for_each_new_crtc_state(|object, opaque| {
                visited += 1;
                core::hint::black_box(opaque);
                assert!(state.get_new_crtc_state(object).is_none());
                assert!(matches!(state.add_crtc_state(object), Err(EBUSY)));
                assert_eq!(state.try_for_each_new_crtc_state(|_, _| {}), Err(EBUSY));
            })?;
            assert_eq!(visited, 1);
            assert!(state.get_new_crtc_state(crtc).is_some());
            Err(ECANCELED)
        };
        // SAFETY: This task owns the initialized, unregistered device; no teardown races it.
        let result = unsafe { atomic::run_update(&dev, update) };
        assert_eq!(result, Err(ECANCELED));
        Ok(())
    }

    #[test]
    fn connector_check_inspection_excludes_mutation() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-inspect-connector", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        // SAFETY: The initialized device owns both observed mode objects.
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
            state.as_mut().set_crtc_config(crtc, Some(&scanout))?;
            let guard = state.add_connector_state(connector)?;
            assert_eq!(
                state.with_new_connector_state_for_crtc(crtc, |_| ()),
                Err(EBUSY)
            );
            drop(guard);
            let observed = state.with_new_connector_state_for_crtc(crtc, |new| {
                assert!(new.is_some());
                assert!(state.get_new_connector_state(connector).is_none());
                assert!(matches!(state.add_connector_state(connector), Err(EBUSY)));
                assert_eq!(
                    state.with_new_connector_state_for_crtc(crtc, |_| ()),
                    Err(EBUSY)
                );
                Err::<(), _>(ECANCELED)
            })?;
            assert_eq!(observed, Err(ECANCELED));
            assert!(state.get_new_connector_state(connector).is_some());
            Err(ECANCELED)
        };
        // SAFETY: This task excludes registration, mode-object creation and teardown.
        let result = unsafe { atomic::run_update(&dev, update) };
        assert_eq!(result, Err(ECANCELED));
        Ok(())
    }
}
