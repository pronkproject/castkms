// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::renderer::job::Completion;

#[kunit_tests(rust_castkms_backend_sources)]
mod cases {
    use super::*;

    #[test]
    fn retained_control_cannot_publish_after_endpoint_release() -> Result {
        let display = CastKms::new_constraints(c"castkms-publication-source-revoke", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let (pool, publication) = publication::prepare(device, &owner)?;
            let control = publication.control();
            publication.prepare_reply(|_| Ok(()))?;
            publication.publish(device)?;
            device.atomic_update(|state| {
                state.add_crtc_state(crtc)?.set_constraints(control.entry())
            })?;
            let image = pool.image(1)?;
            let job = control.claim(1, None, image.prepare(1)?)?;
            let hold = crtc.display.output.with_accepted(|accepted| {
                accepted.ok_or(EINVAL)?.source.hold_admission()
            })?;
            check(hold.prepared()?.is_none())?;
            drop(publication);
            let mut published = false;
            check(control.publish_source(&job, || published = true) == Err(EKEYREVOKED))?;
            check(!published && hold.prepared()?.is_none())?;
            job.release(Completion::WithoutAccess);
            check(hold.prepared()?.is_some())?;
            Ok(())
        })
    }

    #[test]
    fn selected_backend_alone_can_publish_its_claimed_scene() -> Result {
        let display = CastKms::new_constraints(c"castkms-publication-source-binding", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let (pool, first) = publication::prepare(device, &owner)?;
            let (_, second) = publication::prepare(device, &owner)?;
            let first_control = first.control();
            let second_control = second.control();
            first.prepare_reply(|_| Ok(()))?;
            first.publish(device)?;
            second.prepare_reply(|_| Ok(()))?;
            second.publish(device)?;
            let image = pool.image(1)?;
            check(first_control.claim(1, None, image.prepare(1)?).err() == Some(ESTALE))?;
            device.atomic_update(|state| {
                state.add_crtc_state(crtc)?.set_constraints(first.entry())
            })?;
            let job = first_control.claim(1, None, image.prepare(2)?)?;
            let mut published = false;
            check(second_control.publish_source(&job, || published = true) == Err(EACCES))?;
            check(!published)?;
            first_control.publish_source(&job, || published = true)?;
            check(published)?;
            let rendered = job.release(Completion::Cpu).ok_or(EINVAL)?;
            first_control.check_completed(&rendered)?;
            device.atomic_update(|state| {
                state.add_crtc_state(crtc)?.set_constraints(second.entry())
            })?;
            check(first_control.check_completed(&rendered) == Err(ESTALE))?;
            check(second_control.check_completed(&rendered) == Err(ESTALE))?;
            Ok(())
        })
    }

    #[test]
    fn issuer_revocation_rejects_installation_without_completing_the_read() -> Result {
        let display = CastKms::new_constraints(c"castkms-publication-source-issuer", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let (pool, publication) = publication::prepare(device, &owner)?;
            let control = publication.control();
            publication.prepare_reply(|_| Ok(()))?;
            publication.publish(device)?;
            device.atomic_update(|state| {
                state.add_crtc_state(crtc)?.set_constraints(publication.entry())
            })?;
            let job = control.claim(1, None, pool.image(1)?.prepare(1)?)?;
            let hold = crtc.display.output.with_accepted(|accepted| {
                accepted.ok_or(EINVAL)?.source.hold_admission()
            })?;
            owner.revoke();
            check(control.publish_source(&job, || ()) == Err(EKEYREVOKED))?;
            check(hold.prepared()?.is_none())?;
            job.release(Completion::WithoutAccess);
            check(hold.prepared()?.is_some())?;
            Ok(())
        })
    }
}
