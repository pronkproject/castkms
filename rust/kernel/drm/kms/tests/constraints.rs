// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::drm::constraints::{Description, Domain, Format, OpaqueEntry, Size};
use crtc::RawCrtc;
use plane::RawPlane;

fn entry(domain: &Domain, crtc_id: u32, plane_id: u32) -> Result<ARef<OpaqueEntry>> {
    let size = Size::exact(640, 480);
    let description = Description::new(
        size,
        &[Format::new(
            plane_id,
            fourcc::XRGB8888,
            fourcc::FORMAT_MOD_LINEAR,
            size,
        )],
        &[],
    )?;
    OpaqueEntry::new_stateless(domain, crtc_id, &description)
}

#[kunit_tests(rust_drm_kms_constraints)]
mod cases {
    use super::*;

    #[test]
    fn publication_checks_scope_without_selecting_or_reserving() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.constraints_capacity.store(8, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-kms-constraints-publication", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let output = dev.constraints_output(0)?;
        let crtc_id = dev.crtc()?.object_id();
        let plane_id = dev.plane()?.object_id();
        let initial = output.default_entry().id();
        assert_eq!(initial, counts.constraints_id.load(Ordering::Relaxed));
        assert_eq!(output.selected().id(), initial);
        let target = entry(output.domain(), crtc_id, plane_id)?;
        assert!(matches!(output.lookup(target.id()), Err(ESTALE)));
        output.add(&target)?;
        output.suggest(target.id())?;
        let snapshot = output.snapshot(0)?;
        assert_eq!(snapshot.info().count, 2);
        assert_eq!(snapshot.info().selected_id, initial);
        assert_eq!(snapshot.info().suggested_id, target.id());
        let retained = output.lookup(target.id())?;
        output.withdraw(target.id())?;
        assert!(matches!(output.lookup(target.id()), Err(ESTALE)));
        assert!(matches!(
            output.snapshot(snapshot.info().generation),
            Err(ESTALE)
        ));
        output.forget(target.id())?;
        let foreign = Domain::new(1)?;
        assert_eq!(
            output.add(&*entry(&foreign, crtc_id, plane_id)?),
            Err(EINVAL)
        );
        assert_eq!(
            output.add(&*entry(output.domain(), u32::MAX, plane_id)?),
            Err(EINVAL)
        );
        assert_eq!(
            output.add(&*entry(output.domain(), crtc_id, u32::MAX)?),
            Err(EINVAL)
        );
        output.close();
        assert!(matches!(output.snapshot(0), Err(ESTALE)));
        assert_eq!(output.selected().id(), initial);
        assert_eq!(output.default_entry().id(), initial);
        drop(target);
        drop(output);
        drop(dev);
        assert_eq!(retained.crtc_id(), crtc_id);
        assert_eq!(snapshot.info().count, 2);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn unattached_output_exposes_no_provider_control() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-no-constraints", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        assert!(matches!(dev.constraints_output(0), Err(EOPNOTSUPP)));
        assert!(matches!(dev.constraints_output(1), Err(EINVAL)));
        Ok(())
    }

    #[test]
    fn final_provider_recheck_rejects_before_state_swap() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.constraints_capacity.store(4, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-kms-constraints-recheck", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let fb = framebuffer(dev.device())?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        counts
            .fail_constraints_at_install
            .store(1, Ordering::Relaxed);
        assert_eq!(
            dev.update(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout))),
            Err(EIO)
        );
        assert_eq!(counts.constraints_checks.load(Ordering::Relaxed), 2);
        assert_eq!(counts.install_calls.load(Ordering::Relaxed), 1);
        assert_eq!(counts.install_successes.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_updates.load(Ordering::Relaxed), 0);
        counts
            .fail_constraints_at_install
            .store(0, Ordering::Relaxed);
        counts.fail_constraints.store(0, Ordering::Relaxed);
        dev.update(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        assert_eq!(counts.constraints_checks.load(Ordering::Relaxed), 4);
        assert_eq!(counts.install_successes.load(Ordering::Relaxed), 1);
        drop(fb);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn attached_default_checks_complete_proposal_before_installation() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.constraints_capacity.store(4, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-kms-constraints", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        assert_ne!(counts.constraints_id.load(Ordering::Relaxed), 0);
        let fb = framebuffer(dev.device())?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        dev.check(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        assert_eq!(counts.constraints_checks.load(Ordering::Relaxed), 1);
        assert_eq!(counts.install_successes.load(Ordering::Relaxed), 0);
        counts.fail_constraints.store(1, Ordering::Relaxed);
        assert_eq!(
            dev.update(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout))),
            Err(EIO)
        );
        assert_eq!(counts.install_successes.load(Ordering::Relaxed), 0);
        counts.fail_constraints.store(0, Ordering::Relaxed);
        dev.update(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout)))?;
        // One successful check, one rejected check, then validation and final acceptance.
        assert_eq!(counts.constraints_checks.load(Ordering::Relaxed), 4);
        assert_eq!(counts.install_successes.load(Ordering::Relaxed), 1);
        assert_eq!(counts.plane_updates.load(Ordering::Relaxed), 1);
        // Provider failure must not prevent shutdown of the accepted default.
        counts.fail_constraints.store(1, Ordering::Relaxed);
        drop(fb);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
