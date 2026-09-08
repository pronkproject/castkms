// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Framebuffer assignment history within one atomic transaction.

use super::*;
use connector::AsRawConnector;
use crtc::AsRawCrtc;
use plane::{AsRawPlane, AsRawPlaneStatePrivate, RawPlaneState};

#[kunit_tests(rust_drm_plane_assignments)]
mod tests {
    use super::*;

    #[test]
    fn inherited_framebuffer_is_not_an_assignment() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-plane-inherited", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        // SAFETY: The private initialized device owns the objects throughout both transactions.
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
        let configure = |mut transaction: Pin<&mut atomic::AtomicStateComposer<TestDriver>>| {
            transaction.as_mut().set_crtc_config(crtc, Some(&scanout))?;
            assert!(transaction.add_plane_state(plane)?.framebuffer_was_set());
            Ok(())
        };
        // SAFETY: Setup is complete; registration and teardown are excluded until return.
        unsafe { atomic::run_update(&dev, configure) }?;
        // SAFETY: No concurrent transaction changes the private plane's published state.
        // The matching destructor releases the duplicated state before any assertion.
        let inherited_assignment = unsafe {
            let callbacks = &*(*plane.as_raw()).funcs;
            let duplicate = callbacks.atomic_duplicate_state.unwrap()(plane.as_raw());
            if duplicate.is_null() {
                return Err(ENOMEM);
            }
            let inherited_assignment = (*duplicate).fb_set;
            callbacks.atomic_destroy_state.unwrap()(plane.as_raw(), duplicate);
            inherited_assignment
        };
        assert!(!inherited_assignment);
        let reassign = |transaction: Pin<&mut atomic::AtomicStateComposer<TestDriver>>| {
            let mut state = transaction.add_plane_state(plane)?;
            assert!(!state.framebuffer_was_set());
            assert_eq!(state.framebuffer().ok_or(EINVAL)?.as_raw(), fb.as_raw());
            // SAFETY: The exclusive state guard and framebuffer belong to the same
            // device. The helper retains its own reference without changing routing.
            unsafe { bindings::drm_atomic_set_fb_for_plane(state.as_raw_mut(), fb.as_raw()) };
            assert!(state.framebuffer_was_set());
            drop(state);
            // Reacquiring an existing new state must not erase the assignment.
            assert!(transaction.add_plane_state(plane)?.framebuffer_was_set());
            Ok(())
        };
        // SAFETY: Both operations finish before private-device teardown.
        unsafe { atomic::run_check(&dev, reassign) }?;
        unsafe { atomic::run_update(&dev, |transaction| transaction.set_crtc_config(crtc, None)) }?;
        drop(fb);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn null_assignment_is_recorded() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-plane-null", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        // SAFETY: The private device is initialized and retains the plane through each check.
        let plane =
            unsafe { plane::Plane::<TestPlane>::from_raw(dev.plane.load(Ordering::Relaxed)) };
        let clear = |transaction: Pin<&mut atomic::AtomicStateComposer<TestDriver>>| {
            let mut state = transaction.add_plane_state(plane)?;
            assert!(!state.framebuffer_was_set());
            assert!(state.framebuffer::<TestDriver>().is_none());
            // SAFETY: The state guard owns mutable access; clearing the framebuffer
            // changes no routing or object identity and releases only its reference.
            unsafe {
                bindings::drm_atomic_set_fb_for_plane(state.as_raw_mut(), core::ptr::null_mut())
            };
            assert!(state.framebuffer_was_set());
            Ok(())
        };
        // SAFETY: The initialized device remains private through both checks.
        unsafe { atomic::run_check(&dev, clear) }?;
        let inherited = |transaction: Pin<&mut atomic::AtomicStateComposer<TestDriver>>| {
            assert!(!transaction.add_plane_state(plane)?.framebuffer_was_set());
            Ok(())
        };
        // SAFETY: Same private-device lifetime; check-only did not publish the first state.
        unsafe { atomic::run_check(&dev, inherited) }?;
        Ok(())
    }

    #[test]
    fn canceled_assignment_does_not_reach_the_next_transaction() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-plane-canceled", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let fb = framebuffer(&dev)?;
        // SAFETY: No registration, teardown or mode-object changes overlap the checks.
        let plane =
            unsafe { plane::Plane::<TestPlane>::from_raw(dev.plane.load(Ordering::Relaxed)) };
        let canceled = |transaction: Pin<&mut atomic::AtomicStateComposer<TestDriver>>| {
            let mut state = transaction.add_plane_state(plane)?;
            // SAFETY: Both objects belong to the private device; assignment uses an
            // exclusive state guard and the helper takes its own framebuffer reference.
            unsafe { bindings::drm_atomic_set_fb_for_plane(state.as_raw_mut(), fb.as_raw()) };
            assert!(state.framebuffer_was_set());
            Err(ECANCELED)
        };
        // SAFETY: Setup is complete and no teardown overlaps the check.
        let result = unsafe { atomic::run_check(&dev, canceled) };
        assert_eq!(result, Err(ECANCELED));
        let inherited = |transaction: Pin<&mut atomic::AtomicStateComposer<TestDriver>>| {
            let state = transaction.add_plane_state(plane)?;
            assert!(!state.framebuffer_was_set());
            assert!(state.framebuffer::<TestDriver>().is_none());
            Ok(())
        };
        // SAFETY: The same private-device exclusion holds after cancellation.
        unsafe { atomic::run_check(&dev, inherited) }?;
        drop(fb);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
