// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::renderer::job::Completion;

#[kunit_tests(rust_castkms_renderer_issuer_lifetime)]
mod cases {
    use super::*;

    #[test]
    fn issuer_close_unpins_ready_storage_without_completing_reads() -> Result {
        let display = CastKms::new_constraints(c"castkms-renderer-issuer-close", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = crate::File::issue_renderer_control(file.file(), crtc, connector)?;
            let endpoint = endpoints::prepared(device, &owner)?;
            endpoint.publish(|_| Ok(()))?;
            let output = device.constraints_output(crtc)?;
            let entry = output.lookup(endpoint.constraints_id()?)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;
            let pending = endpoint.begin_source(1)?;
            let id = pending.id();
            pending.publish(|| ())?;
            let hold = crtc.display.output.with_accepted(|accepted| {
                accepted.ok_or(EINVAL)?.source.hold_admission()
            })?;
            drop(file);
            check(endpoint.describe().is_err())?;
            check(hold.prepared()?.is_none())?;
            check(endpoint.unregister_image(1) == Err(EBUSY))?;
            endpoint.release_source(id, Completion::WithoutAccess)?;
            check(hold.prepared()?.is_some())?;
            // Releasing the job alone does not remove offer pins; issuer cleanup must
            // have revoked the worker even while its renderer endpoint remains open.
            endpoint.unregister_image(1)?;
            owner.revoke();
            Ok(())
        })
    }
}
