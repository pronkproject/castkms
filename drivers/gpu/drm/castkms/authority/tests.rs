// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use kernel::sync::Arc;

#[track_caller]
fn check(condition: bool) -> Result {
    if condition {
        Ok(())
    } else {
        let location = core::panic::Location::caller();
        pr_err!(
            "Authority check failed at {}:{}\n",
            location.file(),
            location.line()
        );
        Err(EINVAL)
    }
}

#[kunit_tests(rust_castkms_authority)]
mod cases {
    use super::*;

    #[test]
    fn handoff_does_not_change_an_earlier_snapshot() -> Result {
        let authority = Arc::pin_init(Authority::new(), GFP_KERNEL)?;
        check(authority.snapshot().is_none())?;
        authority.changed(Some(1));
        let first = authority.snapshot();
        authority.changed(None);
        check(authority.snapshot().is_none())?;
        authority.changed(Some(2));
        check(first == Some(1))?;
        check(authority.snapshot() == Some(2))?;
        Ok(())
    }

    #[test]
    fn shutdown_rejects_reacquisition() -> Result {
        let authority = Arc::pin_init(Authority::new(), GFP_KERNEL)?;
        authority.changed(Some(1));
        authority.close();
        authority.changed(Some(2));
        authority.changed(None);
        authority.close();
        check(authority.snapshot().is_none())?;
        check(matches!(*authority.state.lock(), State::Closed))?;
        Ok(())
    }

    struct Reenter(Arc<Authority<Reenter>>);

    impl Drop for Reenter {
        fn drop(&mut self) {
            self.0.close();
        }
    }

    #[test]
    fn removed_identity_is_destroyed_outside_the_lock() -> Result {
        let authority = Arc::pin_init(Authority::new(), GFP_KERNEL)?;
        authority.changed(Some(Reenter(authority.clone())));
        authority.changed(None);
        check(matches!(*authority.state.lock(), State::Closed))?;
        Ok(())
    }

    #[test]
    fn closing_identity_is_destroyed_outside_the_lock() -> Result {
        let authority = Arc::pin_init(Authority::new(), GFP_KERNEL)?;
        authority.changed(Some(Reenter(authority.clone())));
        authority.close();
        check(matches!(*authority.state.lock(), State::Closed))?;
        Ok(())
    }
}
