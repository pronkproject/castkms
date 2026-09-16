// SPDX-License-Identifier: GPL-2.0-only

use super::{
    private_images::{activate, buffer},
    *,
};
use crate::{
    capture::{permission, provider::Grantor},
    renderer::job::Completion,
};
use kernel::drm::gem::ExportAccess;

pub(super) fn grant(
    file: &RegisteredMasterFile<'_, Driver>,
    crtc: &Crtc<display::Crtc>,
    connector: &Connector<display::Connector>,
) -> Result<Grantor> {
    let master = file.file().master_snapshot().ok_or(EINVAL)?;
    let permission = {
        let guard = master.master().lock_current().ok_or(EACCES)?;
        permission::Permission::new(&guard, crtc, connector)?
    };
    Grantor::new(permission)
}

#[kunit_tests(rust_castkms_delegated_authority)]
mod cases {
    use super::*;

    #[test]
    fn capture_revocation_does_not_inherit_renderer_authority() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, execution) = activate(&candidate, device, crtc)?;
            let grantor = grant(&file, crtc, connector)?;
            let scope = grantor.capture().describe_delegated()?;
            check(scope.dimensions() == [640, 480])?;
            scope.with_current(|_| Ok(()))?;
            let image = candidate.register_private_image(
                &active,
                [640, 480],
                &[buffer(device, ExportAccess::ReadWrite)?],
            )?;
            let rendered = candidate
                .claim_render(&active, execution, None, image.prepare(1)?)?
                .release(Completion::Cpu)
                .ok_or(EINVAL)?;
            let observation = active.observation();
            scope.with_image(&candidate, &observation, &rendered, |_| Ok(()))?;
            drop(grantor);
            let mut called = false;
            check(
                scope.with_image(&candidate, &observation, &rendered, |_| {
                    called = true;
                    Ok(())
                }) == Err(EKEYREVOKED),
            )?;
            check(!called)?;
            candidate.with_content(&active, rendered.content(), || Ok(()))
        })
    }

    #[test]
    fn renderer_revocation_does_not_inherit_capture_authority() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, execution) = activate(&candidate, device, crtc)?;
            let grantor = grant(&file, crtc, connector)?;
            let scope = grantor.capture().describe_delegated()?;
            let image = candidate.register_private_image(
                &active,
                [640, 480],
                &[buffer(device, ExportAccess::ReadWrite)?],
            )?;
            let rendered = candidate
                .claim_render(&active, execution, None, image.prepare(1)?)?
                .release(Completion::Cpu)
                .ok_or(EINVAL)?;
            owner.revoke();
            scope.with_current(|_| Ok(()))?;
            check(
                scope.with_image(&candidate, &active.observation(), &rendered, |_| Ok(()))
                    == Err(EKEYREVOKED),
            )
        })
    }

    #[test]
    fn observing_an_active_renderer_does_not_keep_it_active() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, execution) = activate(&candidate, device, crtc)?;
            let grantor = grant(&file, crtc, connector)?;
            let scope = grantor.capture().describe_delegated()?;
            let image = candidate.register_private_image(
                &active,
                [640, 480],
                &[buffer(device, ExportAccess::ReadWrite)?],
            )?;
            let rendered = candidate
                .claim_render(&active, execution, None, image.prepare(1)?)?
                .release(Completion::Cpu)
                .ok_or(EINVAL)?;
            let observation = active.observation();
            drop(active);
            check(scope.with_image(&candidate, &observation, &rendered, |_| Ok(())) == Err(EIO))
        })
    }

    #[test]
    fn delegated_description_does_not_accept_a_reenabled_configuration() -> Result {
        with_display(|device, crtc, connector, scanout, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (_active, _) = activate(&candidate, device, crtc)?;
            let grantor = grant(&file, crtc, connector)?;
            let scope = grantor.capture().describe_delegated()?;
            device.atomic_update(|transaction| transaction.set_crtc_config(crtc, None))?;
            device.atomic_update(|transaction| transaction.set_crtc_config(crtc, Some(scanout)))?;
            check(scope.with_current(|_| Ok(())) == Err(ESTALE))?;
            grantor
                .capture()
                .describe_delegated()?
                .with_current(|_| Ok(()))
        })
    }

    #[test]
    fn capture_for_another_output_does_not_authorize_a_private_image() -> Result {
        let other = CastKms::new(c"castkms-delegated-other-output")?;
        with_display(|device, crtc, connector, _, file| {
            let authority = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(authority.access())?, GFP_KERNEL)?;
            let (active, execution) = activate(&candidate, device, crtc)?;
            let image = candidate.register_private_image(
                &active,
                [640, 480],
                &[buffer(device, ExportAccess::ReadWrite)?],
            )?;
            let rendered = candidate
                .claim_render(&active, execution, None, image.prepare(1)?)?
                .release(Completion::Cpu)
                .ok_or(EINVAL)?;
            with_registered_display(&other, |other, crtc, connector, _, file| {
                let authority = owner(&file, crtc, connector)?;
                let worker = Arc::new(Candidate::begin(authority.access())?, GFP_KERNEL)?;
                let (_other_active, _) = activate(&worker, other, crtc)?;
                let grantor = grant(&file, crtc, connector)?;
                let scope = grantor.capture().describe_delegated()?;
                check(
                    scope.with_image(&candidate, &active.observation(), &rendered, |_| Ok(()))
                        == Err(EACCES),
                )
            })
        })
    }
}
