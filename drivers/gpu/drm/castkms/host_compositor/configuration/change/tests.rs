// SPDX-License-Identifier: GPL-2.0-only

use super::*;

#[kunit_tests(rust_castkms_host_changes)]
mod cases {
    use super::*;

    #[test]
    fn enabling_restores_lazy_allocation_without_constructing_a_worker() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let owner = Owner::new(output)?;
        let configuration = owner.configuration();
        configuration.with_change(|change| {
            change.disable()?;
            change.enable()?;
            change.enable()
        })?;
        assert!(matches!(configuration.current(), Err(EAGAIN)));
        Ok(())
    }

    #[test]
    fn enabling_cannot_reopen_owner_shutdown() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let owner = Owner::new(output)?;
        let configuration = owner.configuration();
        configuration.with_change(|change| change.disable())?;
        owner.close();
        assert_eq!(
            configuration.with_change(|change| change.enable()),
            Err(ENODEV)
        );
        assert!(matches!(configuration.current(), Err(ENODEV)));
        Ok(())
    }

    #[test]
    fn disabling_an_idle_configuration_is_idempotent() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let owner = Owner::new(output)?;
        let configuration = owner.configuration();
        configuration.with_change(|change| {
            change.disable()?;
            assert!(matches!(configuration.current(), Err(EOPNOTSUPP)));
            change.disable()
        })?;
        assert!(matches!(configuration.current(), Err(EOPNOTSUPP)));
        owner.close();
        assert!(matches!(configuration.current(), Err(ENODEV)));
        Ok(())
    }

    #[test]
    fn callback_failure_before_a_change_preserves_configuration() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let owner = Owner::new(output)?;
        let configuration = owner.configuration();
        assert_eq!(configuration.with_change(|_| Err(EACCES)), Err(EACCES));
        assert!(matches!(configuration.current(), Err(EAGAIN)));
        Ok(())
    }

    #[test]
    fn a_later_callback_error_does_not_undo_disabled_admission() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let owner = Owner::new(output)?;
        let configuration = owner.configuration();
        assert_eq!(
            configuration.with_change(|change| {
                change.disable()?;
                Err(EIO)
            }),
            Err(EIO)
        );
        assert!(matches!(configuration.current(), Err(EOPNOTSUPP)));
        Ok(())
    }

    #[test]
    fn closed_configuration_does_not_enter_the_callback() -> Result {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let owner = Owner::new(output)?;
        let configuration = owner.configuration();
        owner.close();
        let mut entered = false;
        assert_eq!(
            configuration.with_change(|_| {
                entered = true;
                Ok(())
            }),
            Err(ENODEV)
        );
        assert!(!entered);
        Ok(())
    }
}
