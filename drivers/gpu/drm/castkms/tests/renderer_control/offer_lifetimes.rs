// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::renderer::{draft::Draft, private_pool::Pool, ready};
use kernel::drm::gem::ExportAccess;

fn prepare(
    device: &Device<Driver, Registered>,
    owner: &Owner,
) -> Result<(Pool, ready::Owner)> {
    let draft = Draft::new(owner.access(), private_images::profile()?, [640, 480])?;
    let mut pool = Pool::new()?;
    pool.insert(1, || draft.register_image(&[
        private_images::buffer(device, ExportAccess::ReadWrite)?,
    ]))?;
    draft.submit_probe(None)?;
    let ready = draft.prepare_worker(&pool)?;
    Ok((pool, ready))
}

fn selectable(snapshot: &kernel::drm::constraints::Snapshot, id: u64) -> Result<bool> {
    snapshot.entries().find(|offer| offer.entry.id() == id)
        .map(|offer| offer.selectable).ok_or(ENOENT)
}

#[kunit_tests(rust_castkms_offer_lifetimes)]
mod cases {
    use super::*;

    #[test]
    fn revocation_withdraws_every_offer_without_rebinding_accepted_state() -> Result {
        let display = CastKms::new_constraints(c"castkms-offer-withdraw", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let (mut pool, ready) = prepare(device, &owner)?;
            let provider = crtc.display.constraints.as_ref().ok_or(EINVAL)?;
            let control = device.constraints_output(crtc)?;
            let first = provider.prepare(ready.worker())?;
            let second = provider.prepare(ready.worker())?;
            provider.publish(&control, &first)?;
            provider.publish(&control, &second)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&first))?;
            control.suggest(second.id())?;
            let retained = control.snapshot(0)?;
            check(selectable(&retained, first.id())? && selectable(&retained, second.id())?)?;
            drop(ready);
            let withdrawn = control.snapshot(0)?;
            check(withdrawn.info().generation == retained.info().generation + 2)?;
            check(withdrawn.info().suggested_id == 0)?;
            check(withdrawn.info().selected_id == first.id())?;
            check(!selectable(&withdrawn, first.id())? && !selectable(&withdrawn, second.id())?)?;
            check(selectable(&withdrawn, provider.initial().id())?)?;
            check(selectable(&retained, first.id())? && selectable(&retained, second.id())?)?;
            check(control.lookup(second.id()).err() == Some(ESTALE))?;
            owner.revoke();
            check(control.snapshot(0)?.info().generation == withdrawn.info().generation)?;
            drop(pool.remove(1)?);
            device.atomic_update(|state| state.set_crtc_config(crtc, None))?;
            control.restore_default()?;
            control.forget(first.id())?;
            control.forget(second.id())?;
            drop(provider.remove(&first));
            drop(provider.remove(&second));
            Ok(())
        })
    }

    #[test]
    fn failed_publication_does_not_acquire_withdrawal_ownership() -> Result {
        let display = CastKms::new_constraints(c"castkms-offer-rollback", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let (mut pool, ready) = prepare(device, &owner)?;
            let provider = crtc.display.constraints.as_ref().ok_or(EINVAL)?;
            let control = device.constraints_output(crtc)?;
            let injected = provider.prepare(ready.worker())?;
            // Inject native membership without going through this worker's publication.
            control.add(&injected)?;
            check(provider.publish(&control, &injected) == Err(EEXIST))?;
            check(provider.resolve(&injected).err() == Some(ESTALE))?;
            let published = provider.prepare(ready.worker())?;
            provider.publish(&control, &published)?;
            drop(ready);
            let snapshot = control.snapshot(0)?;
            check(selectable(&snapshot, injected.id())?)?;
            check(!selectable(&snapshot, published.id())?)?;
            control.withdraw(injected.id())?;
            control.forget(injected.id())?;
            control.forget(published.id())?;
            drop(provider.remove(&published));
            drop(pool.remove(1)?);
            Ok(())
        })
    }

    #[test]
    fn withdrawal_records_are_bounded_without_partial_publication() -> Result {
        let display = CastKms::new_constraints(c"castkms-offer-bound", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let (mut pool, ready) = prepare(device, &owner)?;
            let provider = crtc.display.constraints.as_ref().ok_or(EINVAL)?;
            let control = device.constraints_output(crtc)?;
            for _ in 0..kernel::drm::constraints::List::MAX_ENTRIES {
                let entry = provider.prepare(ready.worker())?;
                provider.publish(&control, &entry)?;
                control.withdraw(entry.id())?;
                control.forget(entry.id())?;
                drop(provider.remove(&entry));
            }
            let overflow = provider.prepare(ready.worker())?;
            check(provider.publish(&control, &overflow) == Err(ENOSPC))?;
            check(provider.resolve(&overflow).err() == Some(ESTALE))?;
            check(control.lookup(overflow.id()).err() == Some(ESTALE))?;
            drop(ready);
            check(control.snapshot(0)?.info().count == 1)?;
            drop(pool.remove(1)?);
            Ok(())
        })
    }

    #[test]
    fn another_workers_guard_cannot_own_publication() -> Result {
        let display = CastKms::new_constraints(c"castkms-offer-guard", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let (_, first) = prepare(device, &owner)?;
            let (_, second) = prepare(device, &owner)?;
            let provider = crtc.display.constraints.as_ref().ok_or(EINVAL)?;
            let control = device.constraints_output(crtc)?;
            let entry = provider.prepare(first.worker())?;
            let worker = second.worker();
            let mut guard = worker.hold_ready()?;
            check(guard.publish(&control, &entry) == Err(EACCES))?;
            check(control.lookup(entry.id()).err() == Some(ESTALE))?;
            drop(guard);
            provider.publish(&control, &entry)?;
            Ok(())
        })
    }
}
