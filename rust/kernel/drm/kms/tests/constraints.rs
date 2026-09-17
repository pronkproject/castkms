// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::drm::constraints::{Description, Domain, Format, OpaqueEntry, Size};
use crtc::{RawCrtc, RawCrtcState};
use plane::RawPlane;

mod provider;
mod properties;
pub(super) use provider::{prepare, publish, verify, Prepared, Published};

fn entry(domain: &Domain, crtc_id: u32, plane_id: u32) -> Result<ARef<OpaqueEntry>> {
    format_entry(domain, crtc_id, plane_id, fourcc::XRGB8888)
}

fn format_entry(
    domain: &Domain,
    crtc_id: u32,
    plane_id: u32,
    format: u32,
) -> Result<ARef<OpaqueEntry>> {
    let size = Size::exact(640, 480);
    let description = Description::new(
        size,
        &[
            Format::new(plane_id, format, fourcc::FORMAT_MOD_LINEAR, size),
            Format::implicit(plane_id, format, size),
        ],
        &[],
    )?;
    OpaqueEntry::new_stateless(domain, crtc_id, &description)
}

#[kunit_tests(rust_drm_kms_constraints)]
mod cases {
    use super::*;

    #[test]
    fn teardown_releases_bindings_but_retained_snapshots_keep_quota() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.constraints_capacity.store(1, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-kms-constraints-retained-quota", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let output = dev.constraints_output(0)?;
        let domain = ARef::from(output.domain());
        let initial = ARef::from(output.default_entry());
        let description = ARef::from(initial.description());
        let snapshot = output.snapshot(0)?;
        let crtc_id = initial.crtc_id();
        let initial_id = initial.id();
        drop(output);
        drop(dev);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert!(matches!(
            OpaqueEntry::new_stateless(&domain, crtc_id, &description),
            Err(ENOSPC)
        ));
        drop(initial);
        assert_eq!(snapshot.info().selected_id, initial_id);
        assert!(matches!(
            OpaqueEntry::new_stateless(&domain, crtc_id, &description),
            Err(ENOSPC)
        ));
        drop(snapshot);
        // Retained metadata grants no device access; allocation here only probes quota release.
        let next = OpaqueEntry::new_stateless(&domain, crtc_id, &description)?;
        assert!(next.id() > initial_id);
        Ok(())
    }

    #[test]
    fn default_quota_failure_unwinds_partially_attached_topology() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.constraints_capacity.store(1, Ordering::Relaxed);
        counts.output_count.store(2, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-kms-constraints-quota-unwind", None)?;
        let result = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?).err();
        assert_eq!(result, Some(ENOSPC));
        assert_ne!(counts.constraints_id.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.crtc_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.plane_states.load(Ordering::Relaxed), 0);
        assert_eq!(counts.connector_states.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn target_buffers_precede_selection_and_binding_reaches_commit_tail() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.constraints_capacity.store(4, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-kms-constraints-selection", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let output = dev.constraints_output(0)?;
        let target = format_entry(
            output.domain(),
            dev.crtc()?.object_id(),
            dev.plane()?.object_id(),
            fourcc::NV12,
        )?;
        assert_eq!(output.add(&target), Ok(()));
        let initial = output.selected().id();
        let initial_fb = framebuffer(dev.device())?;
        let mode = mode()?;
        let initial_scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &initial_fb,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        assert_eq!(
            dev.update(|state| state.set_crtc_config(dev.crtc()?, Some(&initial_scanout))),
            Ok(())
        );
        // Allocation and framebuffer construction admit target-only storage before selection.
        let object = gem::shmem::Object::<TestObject>::new(
            dev.device(),
            crate::page::page_align(640 * 480 * 3 / 2).ok_or(EOVERFLOW)?,
            Default::default(),
            (),
        )?;
        let fb = dev.framebuffer(
            &framebuffer::FramebufferLayout {
                width: 640,
                height: 480,
                format: fourcc::NV12,
                modifier: None,
                interlaced: false,
                planes: &[
                    framebuffer::FramebufferPlane {
                        object: &object,
                        pitch: 640,
                        offset: 0,
                    },
                    framebuffer::FramebufferPlane {
                        object: &object,
                        pitch: 640,
                        offset: 640 * 480,
                    },
                ],
            },
            framebuffers::Metadata::new(&counts, false),
        )?;
        assert_eq!(output.selected().id(), initial);
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[dev.connector()?],
            position: (0, 0),
        };
        assert_eq!(
            dev.check(|state| state.set_crtc_config(dev.crtc()?, Some(&scanout))),
            Err(EINVAL)
        );
        let select = |mut state: Pin<&mut atomic::AtomicStateComposer<TestDriver>>| {
            state
                .as_mut()
                .set_crtc_config(dev.crtc()?, Some(&scanout))?;
            state.add_crtc_state(dev.crtc()?)?.set_constraints(&target)
        };
        assert_eq!(dev.check(select), Ok(()));
        assert_eq!(output.selected().id(), initial);
        assert_eq!(
            counts.committed_constraints_id.load(Ordering::Relaxed),
            initial
        );
        assert_eq!(dev.update(select), Ok(()));
        assert_eq!(output.selected().id(), target.id());
        assert_eq!(
            counts.checked_constraints_id.load(Ordering::Relaxed),
            target.id()
        );
        assert_eq!(
            counts.committed_constraints_id.load(Ordering::Relaxed),
            target.id()
        );
        let generation = output.snapshot(0)?.info().generation;
        assert_eq!(dev.update(select), Ok(()));
        dev.update(|state| {
            let new = state.add_crtc_state(dev.crtc()?)?;
            assert_eq!(
                new.constraints_entry().map(|entry| entry.id()),
                Some(target.id())
            );
            Ok(())
        })?;
        assert_eq!(output.snapshot(0)?.info().generation, generation);
        output.withdraw(target.id())?;
        assert_eq!(dev.update(select), Ok(()));
        assert_eq!(
            counts.committed_constraints_id.load(Ordering::Relaxed),
            target.id()
        );
        output.close();
        drop(output);
        drop(fb);
        drop(initial_fb);
        drop(object);
        drop(dev);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn checked_offer_withdrawal_preserves_accepted_binding() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        counts.constraints_capacity.store(4, Ordering::Relaxed);
        let parent = faux::Registration::new(c"rust-kms-constraints-stale", None)?;
        let dev = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let output = dev.constraints_output(0)?;
        let target = entry(
            output.domain(),
            dev.crtc()?.object_id(),
            dev.plane()?.object_id(),
        )?;
        let initial = output.selected().id();
        let select = |state: Pin<&mut atomic::AtomicStateComposer<TestDriver>>| {
            state.add_crtc_state(dev.crtc()?)?.set_constraints(&target)
        };
        assert_eq!(dev.check(select), Err(ESTALE));
        output.add(&target)?;
        dev.check(select)?;
        output.withdraw(target.id())?;
        assert_eq!(dev.update(select), Err(ESTALE));
        assert_eq!(output.selected().id(), initial);
        assert_eq!(counts.install_successes.load(Ordering::Relaxed), 0);
        Ok(())
    }

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
        output.add_suggested(&target)?;
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
