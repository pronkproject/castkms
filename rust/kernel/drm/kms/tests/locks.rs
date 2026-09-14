// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Control callbacks observe one device's modeset state without transaction retries.

use super::*;
use crtc::AsRawCrtc;

#[kunit_tests(rust_drm_modeset_locks)]
mod cases {
    use super::*;

    #[test]
    fn control_checks_device_identity_and_runs_once() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-modeset-control", None)?;
        let foreign_parent = faux::Registration::new(c"rust-modeset-other", None)?;
        let foreign = testing::TestDevice::new(allocate(foreign_parent.as_ref(), &counts, false)?)?;
        // SAFETY: Registration is destroyed before its retained faux parent on every exit.
        let registration = unsafe {
            drm::Registration::new_static(
                parent.as_ref().as_ref(),
                allocate(parent.as_ref(), &counts, false)?,
                Ok::<(), Error>(()),
                0,
            )?
        };
        let (result, calls) = {
            let registered = registration.registration_guard().ok_or(ENODEV)?;
            // SAFETY: Setup records this device's CRTC, whose teardown registration excludes.
            let crtc = unsafe {
                crtc::Crtc::<TestCrtc>::from_raw(registered.crtc.load(Ordering::Relaxed))
            };
            let mut calls = 0;
            let result = registered.with_modeset_locks(|state| {
                calls += 1;
                if !matches!(state.preparation_source(foreign.crtc()?), Err(EINVAL))
                    || state.preparation_source(crtc)?.is_some()
                {
                    return Err(EINVAL);
                }
                // SAFETY: The view holds both locks. Their context pointers remain stable
                // until the callback ends, independently of whether CRTC state exists yet.
                let (crtc_context, connection_context) = unsafe {
                    (
                        (*crtc.as_raw()).mutex.mutex.ctx,
                        (*registered.as_raw())
                            .mode_config
                            .connection_mutex
                            .mutex
                            .ctx,
                    )
                };
                if crtc_context.is_null() || crtc_context != connection_context {
                    return Err(EINVAL);
                }
                Err::<(), _>(EIO)
            })?;
            // Successful reacquisition establishes that returning an error released the locks.
            registered.with_modeset_locks(|_| calls += 1)?;
            (result, calls)
        };
        drop(registration);
        drop(foreign);
        assert_eq!(result, Err(EIO));
        assert_eq!(calls, 2);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
