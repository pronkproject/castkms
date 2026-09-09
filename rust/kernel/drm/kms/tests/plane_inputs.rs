// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Plane input identity across validation, mutation and transaction reuse.

use super::*;
use crate::error::to_result;
use atomic::PlaneInput;
use connector::AsRawConnector;
use crtc::{AsRawCrtc, AsRawCrtcStatePrivate};
use plane::{AsRawPlane, AsRawPlaneStatePrivate, RawPlaneState};

#[kunit_tests(rust_drm_plane_inputs)]
mod tests {
    use super::*;

    #[test]
    fn helper_added_plane_remains_omitted() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-plane-input-helper", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        // SAFETY: The private initialized device retains all three objects until teardown.
        let plane =
            unsafe { plane::Plane::<TestPlane>::from_raw(dev.plane.load(Ordering::Relaxed)) };
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
        // SAFETY: Setup is complete and teardown cannot overlap the synchronous update.
        unsafe {
            atomic::run_update(&dev, |transaction| {
                transaction.set_crtc_config(crtc, Some(&scanout))
            })
        }?;
        let check = |transaction: Pin<&mut atomic::AtomicStateComposer<TestDriver>>| {
            let state = transaction.add_crtc_state(crtc)?;
            // SAFETY: The guard owns exclusive CRTC state access. Requesting a full
            // modeset makes the native helpers include its unchanged active plane.
            unsafe { (*state.as_raw()).set_mode_changed(true) };
            drop(state);
            // SAFETY: No state guard overlaps native validation.
            to_result(unsafe { bindings::drm_atomic_check_only(transaction.as_raw()) })?;
            assert!(matches!(
                transaction.plane_input(plane)?,
                PlaneInput::Omitted
            ));
            assert!(transaction.get_new_plane_state(plane).is_some());
            Err(ECANCELED)
        };
        // SAFETY: The initialized device remains private throughout both operations.
        assert_eq!(unsafe { atomic::run_check(&dev, check) }, Err(ECANCELED));
        unsafe { atomic::run_update(&dev, |transaction| transaction.set_crtc_config(crtc, None)) }?;
        drop(fb);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn failed_validation_retains_the_original_framebuffer() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-plane-input", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        // SAFETY: The private initialized device retains its plane throughout the check.
        let plane =
            unsafe { plane::Plane::<TestPlane>::from_raw(dev.plane.load(Ordering::Relaxed)) };
        let check = |transaction: Pin<&mut atomic::AtomicStateComposer<TestDriver>>| {
            let fb = framebuffer(&dev)?;
            let identity = fb.as_raw();
            assert!(matches!(
                transaction.plane_input(plane)?,
                PlaneInput::Uncaptured
            ));
            let mut state = transaction.add_plane_state(plane)?;
            // SAFETY: The exclusive guard and framebuffer belong to the same device.
            // No CRTC is assigned, so native validation will reject the incomplete state.
            unsafe { bindings::drm_atomic_set_fb_for_plane(state.as_raw_mut(), fb.as_raw()) };
            drop(state);
            // SAFETY: No state guard is outstanding; the runner owns the acquire context.
            match to_result(unsafe { bindings::drm_atomic_check_only(transaction.as_raw()) }) {
                Err(error) if error == EINVAL => (),
                Err(error) => return Err(error),
                Ok(()) => return Err(EINVAL),
            }
            let mut state = transaction.add_plane_state(plane)?;
            // SAFETY: Only the guard has mutable access to the new state.
            unsafe {
                bindings::drm_atomic_set_fb_for_plane(state.as_raw_mut(), core::ptr::null_mut())
            };
            // Input inspection must coexist with an exclusive new-state guard.
            match transaction.plane_input(plane)? {
                PlaneInput::Included {
                    framebuffer: Some(input),
                    framebuffer_assigned: true,
                } => {
                    assert_eq!(input.as_raw(), fb.as_raw());
                }
                _ => return Err(EINVAL),
            }
            assert!(state.framebuffer::<TestDriver>().is_none());
            drop(state);
            drop(fb);
            assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 1);
            // SAFETY: All state guards have been released before repeat validation.
            to_result(unsafe { bindings::drm_atomic_check_only(transaction.as_raw()) })?;
            match transaction.plane_input(plane)? {
                PlaneInput::Included {
                    framebuffer: Some(input),
                    framebuffer_assigned: true,
                } => {
                    assert_eq!(input.as_raw(), identity);
                }
                _ => return Err(EINVAL),
            }
            Err(ECANCELED)
        };
        // SAFETY: Setup is complete and no registration or teardown overlaps the callback.
        assert_eq!(unsafe { atomic::run_check(&dev, check) }, Err(ECANCELED));
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn clear_discards_input_before_reconstruction() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-plane-input-clear", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        // SAFETY: The private device owns the plane throughout the callback.
        let plane =
            unsafe { plane::Plane::<TestPlane>::from_raw(dev.plane.load(Ordering::Relaxed)) };
        let check = |transaction: Pin<&mut atomic::AtomicStateComposer<TestDriver>>| {
            // SAFETY: The empty transaction is exclusively controlled by the callback.
            to_result(unsafe { bindings::drm_atomic_check_only(transaction.as_raw()) })?;
            assert!(matches!(
                transaction.plane_input(plane)?,
                PlaneInput::Omitted
            ));
            // SAFETY: No guards or borrowed inputs survive clear. The acquire context
            // remains owned by the runner while the request is reconstructed.
            unsafe { bindings::drm_atomic_commit_clear(transaction.as_raw()) };
            assert!(matches!(
                transaction.plane_input(plane)?,
                PlaneInput::Uncaptured
            ));
            drop(transaction.add_plane_state(plane)?);
            // SAFETY: No guard overlaps validation of the reconstructed transaction.
            to_result(unsafe { bindings::drm_atomic_check_only(transaction.as_raw()) })?;
            assert!(matches!(
                transaction.plane_input(plane)?,
                PlaneInput::Included {
                    framebuffer: None,
                    framebuffer_assigned: false,
                }
            ));
            Err(ECANCELED)
        };
        // SAFETY: The initialized device remains private until the runner returns.
        assert_eq!(unsafe { atomic::run_check(&dev, check) }, Err(ECANCELED));
        Ok(())
    }

    #[test]
    fn another_devices_plane_is_rejected() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-plane-input-device", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let other = create(parent.as_ref(), &counts, false)?;
        // SAFETY: The other private device retains the plane through the callback.
        let plane =
            unsafe { plane::Plane::<TestPlane>::from_raw(other.plane.load(Ordering::Relaxed)) };
        let check = |transaction: Pin<&mut atomic::AtomicStateComposer<TestDriver>>| {
            assert!(matches!(transaction.plane_input(plane), Err(EINVAL)));
            Ok(())
        };
        // SAFETY: Both devices are initialized and private throughout the operation.
        unsafe { atomic::run_check(&dev, check) }
    }
}
