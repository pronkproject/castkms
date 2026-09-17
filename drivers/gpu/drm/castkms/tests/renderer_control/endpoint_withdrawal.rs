// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::renderer::{endpoint::{Endpoint, Phase}, job::Completion};
use kernel::dma_fence::{testing::ManualFence, Status};

#[kunit_tests(rust_castkms_endpoint_withdrawal)]
mod cases {
    use super::*;

    #[test]
    fn withdrawal_preserves_release_channel() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-withdraw-read", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = endpoints::prepared(device, &owner)?;
            check(endpoint.describe()?.phase == Phase::Configured)?;
            endpoint.publish(None, |_| Ok(()))?;
            let description = endpoint.describe()?;
            check(description.phase == Phase::Published)?;
            let output = device.constraints_output(crtc)?;
            let entry = output.lookup(description.constraints_id)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;
            let pending = endpoint.begin_source(1)?;
            let id = pending.id();
            pending.publish(|| ())?;
            let hold = crtc.display.output.with_accepted(|accepted| {
                accepted.ok_or(EINVAL)?.source.hold_admission()
            })?;
            endpoint.withdraw()?;
            endpoint.withdraw()?;
            let withdrawn = endpoint.describe()?;
            check(withdrawn.phase == Phase::Withdrawn)?;
            check(withdrawn.constraints_id == description.constraints_id)?;
            check(output.snapshot(0)?.info().selected_id == entry.id())?;
            check(hold.prepared()?.is_none())?;
            check(endpoint.unregister_image(1) == Err(EBUSY))?;
            let mut native = ManualFence::new()?;
            endpoint.release_source(id, Completion::Submitted(native.fence()))?;
            let prepared = hold.prepared()?.ok_or(EINVAL)?;
            let completion = prepared.completion()?.ok_or(EINVAL)?;
            check(completion.status() == Status::Pending)?;
            check(endpoint.begin_source(1).err() == Some(EKEYREVOKED))?;
            endpoint.unregister_image(1)?;
            check(completion.status() == Status::Pending)?;
            native.complete(Ok(()))?;
            check(completion.status() == Status::Complete(Ok(())))?;
            check(endpoint.publish(None, |_| Ok(())) == Err(EALREADY))?;
            check(endpoint.declare(private_images::profile()?, [640, 480]) == Err(EALREADY))?;
            Ok(())
        })
    }

    #[test]
    fn withdrawal_rejects_pending_installation() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-withdraw-pending", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let first = endpoints::prepared(device, &owner)?;
            let second = endpoints::prepared(device, &owner)?;
            first.publish(None, |_| Ok(()))?;
            second.publish(None, |_| Ok(()))?;
            let output = device.constraints_output(crtc)?;
            let entry = output.lookup(first.constraints_id()?)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;
            let pending = first.begin_source(1)?;
            first.withdraw()?;
            let mut installed = false;
            check(pending.publish(|| installed = true) == Err(EKEYREVOKED))?;
            check(!installed)?;
            first.unregister_image(1)?;
            check(first.describe()?.phase == Phase::Withdrawn)?;
            check(second.describe()?.phase == Phase::Published)?;
            check(output.lookup(second.constraints_id()?).is_ok())?;
            check(output.snapshot(0)?.info().selected_id == entry.id())?;
            Ok(())
        })
    }

    #[test]
    fn unpublished_configurations_have_no_backend_to_withdraw() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-withdraw-configuration", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = Endpoint::new(owner.access(), device.to_registered_ref())?;
            check(endpoint.describe()?.phase == Phase::Empty)?;
            check(endpoint.withdraw() == Err(ENODATA))?;
            endpoint.declare(private_images::profile()?, [640, 480])?;
            check(endpoint.withdraw() == Err(ENODATA))?;
            check(endpoint.describe()?.phase == Phase::Configured)?;
            endpoint.close();
            check(endpoint.describe().err() == Some(EKEYREVOKED))?;
            check(endpoint.withdraw() == Err(EKEYREVOKED))?;
            Ok(())
        })
    }
}
