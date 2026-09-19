// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::renderer::{configuration::Configuration, publication::Publication, private_pool::Pool};
use kernel::drm::gem::ExportAccess;

pub(super) fn prepare(
    device: &Device<Driver, Registered>,
    owner: &Owner,
) -> Result<(Pool, Publication)> {
    let configuration = Configuration::new(owner.access(), private_images::profile()?, [640, 480])?;
    let mut pool = Pool::new()?;
    pool.insert(1, || configuration.register_image(&[
        private_images::buffer(device, ExportAccess::ReadWrite)?,
    ]))?;
    let publication = Publication::new(device, &configuration, &pool, None)?;
    Ok((pool, publication))
}

#[kunit_tests(rust_castkms_publication)]
mod cases {
    use super::*;

    #[test]
    fn reply_failure_leaves_no_native_backend_or_selection() -> Result {
        let display = CastKms::new_constraints(c"castkms-publication-reply", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let (mut pool, publication) = prepare(device, &owner)?;
            let output = device.constraints_output(crtc)?;
            let before = output.snapshot(0)?;
            check(publication.prepare_reply(|id| {
                check(id == publication.entry().id())?;
                Err(EFAULT)
            }) == Err(EFAULT))?;
            check(output.snapshot(0)?.info().generation == before.info().generation)?;
            check(output.lookup(publication.entry().id()).err() == Some(ESTALE))?;
            publication.prepare_reply(|_| Ok(()))?;
            publication.publish(device)?;
            check(output.snapshot(0)?.info().selected_id == before.info().selected_id)?;
            device.atomic_update(|state| {
                state.add_crtc_state(crtc)?.set_constraints(publication.entry())
            })?;
            check(output.snapshot(0)?.info().selected_id == publication.entry().id())?;
            drop(publication);
            drop(pool.remove(1)?);
            Ok(())
        })
    }

    #[test]
    fn issuer_revocation_prevents_reply_and_publication() -> Result {
        let display = CastKms::new_constraints(c"castkms-publication-issuer", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let (mut pool, publication) = prepare(device, &owner)?;
            owner.revoke();
            let mut replied = false;
            check(publication.prepare_reply(|_| { replied = true; Ok(()) }) == Err(EKEYREVOKED))?;
            check(!replied)?;
            check(publication.publish(device) == Err(EKEYREVOKED))?;
            check(device.constraints_output(crtc)?.snapshot(0)?.info().count == 1)?;
            drop(pool.remove(1)?);
            Ok(())
        })
    }

    #[test]
    fn disabled_backends_are_independent_and_endpoint_drop_withdraws() -> Result {
        let display = CastKms::new_constraints(c"castkms-publication-endpoint", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            device.atomic_update(|state| state.set_crtc_config(crtc, None))?;
            let (_, first) = prepare(device, &owner)?;
            let (_, second) = prepare(device, &owner)?;
            let first_entry = first.entry().clone();
            first.prepare_reply(|_| Ok(()))?;
            first.publish(device)?;
            second.prepare_reply(|_| Ok(()))?;
            second.publish(device)?;
            let output = device.constraints_output(crtc)?;
            check(output.snapshot(0)?.info().count == 3)?;
            drop(first);
            check(output.lookup(first_entry.id()).err() == Some(ESTALE))?;
            check(output.lookup(second.entry().id()).is_ok())?;
            check(output.snapshot(0)?.info().selected_id == output.default_entry().id())?;
            drop(second);
            check(crtc.display.constraints.as_ref().ok_or(EINVAL)?.reap(&output)? == 0)?;
            check(output.snapshot(0)?.info().count == 1)?;
            Ok(())
        })
    }
}
