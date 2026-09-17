// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::renderer::{configuration::Configuration, private_pool::Pool};
use kernel::drm::gem::ExportAccess;

#[kunit_tests(rust_castkms_backend_reaping)]
mod cases {
    use super::*;

    #[test]
    fn selected_withdrawn_entries_survive_until_default_restoration() -> Result {
        let display = CastKms::new_constraints(c"castkms-reap-selected", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let (_, ready) = backend_lifetimes::prepare(device, &owner)?;
            let provider = crtc.display.constraints.as_ref().ok_or(EINVAL)?;
            let control = device.constraints_output(crtc)?;
            let first = provider.prepare(ready.worker())?;
            let second = provider.prepare(ready.worker())?;
            provider.publish(&control, &first)?;
            provider.publish(&control, &second)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&first))?;
            let retained = control.snapshot(0)?;
            drop(ready);
            check(provider.reap(&control)? == 1)?;
            check(control.snapshot(0)?.info().count == 2)?;
            check(provider.resolve(&second).err() == Some(ESTALE))?;
            check(core::ptr::eq(&*provider.resolve(&first)?, &**first))?;
            device.atomic_update(|state| state.set_crtc_config(crtc, None))?;
            control.restore_default()?;
            check(provider.reap(&control)? == 1)?;
            check(provider.resolve(&first).err() == Some(ESTALE))?;
            check(control.snapshot(0)?.info().count == 1)?;
            check(retained.info().selected_id == first.id() && retained.info().count == 3)?;
            check(provider.reap(&control)? == 0)?;
            Ok(())
        })
    }

    #[test]
    fn forgetting_one_entry_does_not_revoke_its_shared_worker() -> Result {
        let display = CastKms::new_constraints(c"castkms-reap-shared", 2)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let (_, ready) = backend_lifetimes::prepare(device, &owner)?;
            let provider = crtc.display.constraints.as_ref().ok_or(EINVAL)?;
            let control = device.constraints_output(crtc)?;
            let first = provider.prepare(ready.worker())?;
            let second = provider.prepare(ready.worker())?;
            provider.publish(&control, &first)?;
            provider.publish(&control, &second)?;
            let other = device.constraints_output(file.crtc_at(1)?)?;
            check(provider.reap(&other) == Err(EINVAL))?;
            control.withdraw(first.id())?;
            control.forget(first.id())?;
            check(provider.reap(&control)? == 1)?;
            check(provider.resolve(&first).err() == Some(ESTALE))?;
            drop(ready.worker().hold_ready()?);
            check(core::ptr::eq(&*provider.resolve(&second)?, &**second))?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&second))?;
            Ok(())
        })
    }

    #[test]
    fn repeated_retirement_reclaims_index_and_native_domain_capacity() -> Result {
        let display = CastKms::new_constraints(c"castkms-reap-capacity", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let configuration = Configuration::new(
                owner.access(), private_images::profile()?, [640, 480],
            )?;
            let mut pool = Pool::new()?;
            pool.insert(1, || configuration.register_image(&[
                private_images::buffer(device, ExportAccess::ReadWrite)?,
            ]))?;
            let provider = crtc.display.constraints.as_ref().ok_or(EINVAL)?;
            let control = device.constraints_output(crtc)?;
            let mut last = provider.initial().id();
            for _ in 0..crate::execution::constraints::provider::CAPACITY + 8 {
                let ready = configuration.prepare_worker(&pool, None)?;
                let entry = provider.prepare(ready.worker())?;
                check(entry.id() > last)?;
                last = entry.id();
                provider.publish(&control, &entry)?;
                drop(ready);
                check(provider.reap(&control)? == 1)?;
                check(provider.resolve(&entry).err() == Some(ESTALE))?;
                check(control.snapshot(0)?.info().count == 1)?;
            }
            drop(pool.remove(1)?);
            provider.close();
            check(provider.reap(&control) == Err(ESHUTDOWN))?;
            Ok(())
        })
    }

    #[test]
    fn native_owner_recovery_can_forget_entries_before_index_cleanup() -> Result {
        use kernel::time::{delay::fsleep, Delta, Instant, Monotonic};

        let display = CastKms::new_constraints(c"castkms-reap-recovery", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let (_, ready) = backend_lifetimes::prepare(device, &owner)?;
            let provider = crtc.display.constraints.as_ref().ok_or(EINVAL)?;
            let control = device.constraints_output(crtc)?;
            let entry = provider.prepare(ready.worker())?;
            provider.publish(&control, &entry)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;
            drop(file);
            let start = Instant::<Monotonic>::now();
            loop {
                let snapshot = control.snapshot(0)?;
                if snapshot.info().count == 1
                    && snapshot.info().selected_id == provider.initial().id()
                {
                    break;
                }
                if start.elapsed() > Delta::from_secs(2) {
                    return Err(ETIMEDOUT);
                }
                fsleep(Delta::from_millis(1));
            }
            check(provider.reap(&control)? == 1)?;
            check(provider.resolve(&entry).err() == Some(ESTALE))?;
            check(owner.access().with_current(|_| Ok(())).is_err())?;
            Ok(())
        })
    }
}
