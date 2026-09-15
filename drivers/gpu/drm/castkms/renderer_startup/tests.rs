// SPDX-License-Identifier: GPL-2.0-only

//! Bounded candidate control excludes cancellation without claiming a source.

use super::*;

fn owner() -> Result<Owner> {
    let output = Arc::pin_init(crate::output::Output::<()>::new(), GFP_KERNEL)?;
    Owner::new(output.identity())
}

#[kunit_tests(rust_castkms_startup_control)]
mod cases {
    use super::*;

    #[test]
    fn control_holds_the_reservation_lock_until_callback_return() -> Result {
        let owner = owner()?;
        let startup = owner.startup();
        let candidate = startup.begin()?;
        let mut calls = 0;
        let value = candidate.with_current(|| {
            calls += 1;
            assert!(startup.state.try_lock().is_none());
            Ok(27)
        })?;
        assert_eq!(value, 27);
        assert_eq!(calls, 1);
        assert!(startup.state.try_lock().is_some());
        candidate.check()?;
        Ok(())
    }

    #[test]
    fn callback_failure_preserves_the_candidate_and_releases_exclusion() -> Result {
        let owner = owner()?;
        let startup = owner.startup();
        let candidate = startup.begin()?;
        assert_eq!(candidate.with_current(|| Err::<(), _>(EIO)), Err(EIO));
        assert!(startup.state.try_lock().is_some());
        candidate.check()?;
        candidate.cancel();
        let _replacement = startup.begin()?;
        Ok(())
    }

    #[test]
    fn canceled_and_closed_candidates_never_enter_the_callback() -> Result {
        let owner = owner()?;
        let startup = owner.startup();
        let candidate = startup.begin()?;
        candidate.cancel();
        let replacement = startup.begin()?;
        let mut calls = 0;
        assert_eq!(
            candidate.with_current(|| {
                calls += 1;
                Ok(())
            }),
            Err(ECANCELED)
        );
        replacement.check()?;
        owner.close();
        assert_eq!(
            replacement.with_current(|| {
                calls += 1;
                Ok(())
            }),
            Err(ENODEV)
        );
        assert_eq!(calls, 0);
        Ok(())
    }

    #[test]
    fn failed_activation_preserves_the_candidate_reservation() -> Result {
        let owner = owner()?;
        let startup = owner.startup();
        let candidate = startup.begin()?;
        assert!(matches!(candidate.activate(|| Err::<(), _>(EIO)), Err(EIO)));
        candidate.check()?;
        candidate.cancel();
        let _replacement = startup.begin()?;
        Ok(())
    }

    #[test]
    fn active_renderer_ownership_never_reopens_candidate_admission() -> Result {
        let owner = owner()?;
        let startup = owner.startup();
        let candidate = startup.begin()?;
        let (active, value) = candidate.activate(|| Ok(19))?;
        assert_eq!(value, 19);
        candidate.cancel();
        active.check()?;
        assert!(matches!(startup.begin(), Err(EBUSY)));
        drop(active);
        assert!(matches!(startup.begin(), Err(EBUSY)));
        Ok(())
    }

    #[test]
    fn display_interval_change_marks_an_active_renderer_lost() -> Result {
        let owner = owner()?;
        let startup = owner.startup();
        let candidate = startup.begin()?;
        let (active, ()) = candidate.activate(|| Ok(()))?;
        startup.invalidate_current();
        assert_eq!(active.check(), Err(EIO));
        assert!(matches!(startup.begin(), Err(EBUSY)));
        owner.close();
        assert_eq!(active.check(), Err(ENODEV));
        Ok(())
    }
}
