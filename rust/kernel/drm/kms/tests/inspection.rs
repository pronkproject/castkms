// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
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
}
