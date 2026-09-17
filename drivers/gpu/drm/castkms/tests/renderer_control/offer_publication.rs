// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::renderer::{draft::Draft, offer::Offer, private_pool::Pool};
use kernel::drm::gem::ExportAccess;

fn prepare(
    device: &Device<Driver, Registered>,
    owner: &Owner,
) -> Result<(Pool, Offer)> {
    let draft = Draft::new(owner.access(), private_images::profile()?, [640, 480])?;
    let mut pool = Pool::new()?;
    pool.insert(1, || draft.register_image(&[
        private_images::buffer(device, ExportAccess::ReadWrite)?,
    ]))?;
    draft.submit_probe(None)?;
    let offer = Offer::new(device, &draft, &pool)?;
    Ok((pool, offer))
}

#[kunit_tests(rust_castkms_offer_publication)]
mod cases {
    use super::*;

    #[test]
    fn reply_failure_leaves_no_native_offer_or_selection() -> Result {
        let display = CastKms::new_constraints(c"castkms-offer-reply", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let (mut pool, offer) = prepare(device, &owner)?;
            let output = device.constraints_output(crtc)?;
            let before = output.snapshot(0)?;
            check(offer.publish(device, |id| {
                check(id == offer.entry().id())?;
                Err(EFAULT)
            }) == Err(EFAULT))?;
            check(output.snapshot(0)?.info().generation == before.info().generation)?;
            check(output.lookup(offer.entry().id()).err() == Some(ESTALE))?;
            offer.publish(device, |_| Ok(()))?;
            check(output.snapshot(0)?.info().selected_id == before.info().selected_id)?;
            device.atomic_update(|state| {
                state.add_crtc_state(crtc)?.set_constraints(offer.entry())
            })?;
            check(output.snapshot(0)?.info().selected_id == offer.entry().id())?;
            drop(offer);
            drop(pool.remove(1)?);
            Ok(())
        })
    }

    #[test]
    fn issuer_revocation_prevents_reply_and_publication() -> Result {
        let display = CastKms::new_constraints(c"castkms-offer-issuer", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let (mut pool, offer) = prepare(device, &owner)?;
            owner.revoke();
            let mut replied = false;
            check(offer.publish(device, |_| { replied = true; Ok(()) }) == Err(EKEYREVOKED))?;
            check(!replied)?;
            check(device.constraints_output(crtc)?.snapshot(0)?.info().count == 1)?;
            drop(pool.remove(1)?);
            Ok(())
        })
    }

    #[test]
    fn disabled_offers_are_independent_and_endpoint_drop_withdraws() -> Result {
        let display = CastKms::new_constraints(c"castkms-offer-endpoint", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            device.atomic_update(|state| state.set_crtc_config(crtc, None))?;
            let (_, first) = prepare(device, &owner)?;
            let (_, second) = prepare(device, &owner)?;
            let first_entry = first.entry().clone();
            first.publish(device, |_| Ok(()))?;
            second.publish(device, |_| Ok(()))?;
            let output = device.constraints_output(crtc)?;
            check(output.snapshot(0)?.info().count == 3)?;
            drop(first);
            check(output.lookup(first_entry.id()).err() == Some(ESTALE))?;
            check(output.lookup(second.entry().id()).is_ok())?;
            check(output.snapshot(0)?.info().selected_id == output.default_entry().id())?;
            drop(second);
            check(crtc.display.constraints.as_ref().ok_or(EINVAL)?.reap(&output)? == 2)?;
            check(output.snapshot(0)?.info().count == 1)?;
            Ok(())
        })
    }
}
