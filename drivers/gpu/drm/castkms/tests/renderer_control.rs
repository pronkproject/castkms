// SPDX-License-Identifier: GPL-2.0-only

//! Renderer control on registered displays, with real master and atomic callbacks.

mod publication;
mod private_images;

use super::*;
use crate::renderer::{
    candidate::Candidate,
    permission::{
        Owner,
        Permission, //
    }, //
};
use kernel::drm::{
    device::Registered,
    kms::{
        connector::Connector,
        crtc::Crtc,
        framebuffer::Framebuffer,
        testing::RegisteredMasterFile, //
    },
    Device, //
};

pub(super) fn with_display(
    f: impl FnOnce(
        &Device<Driver, Registered>,
        &Crtc<display::Crtc>,
        &Connector<display::Connector>,
        &CrtcScanout<'_, Driver>,
        RegisteredMasterFile<'_, Driver>,
    ) -> Result,
) -> Result {
    let display = CastKms::new(c"castkms-renderer-control")?;
    with_registered_display(&display, f)
}

fn with_registered_display(
    display: &CastKms,
    f: impl FnOnce(
        &Device<Driver, Registered>,
        &Crtc<display::Crtc>,
        &Connector<display::Connector>,
        &CrtcScanout<'_, Driver>,
        RegisteredMasterFile<'_, Driver>,
    ) -> Result,
) -> Result {
    let registered = display._display.registration_guard().ok_or(ENODEV)?;
    let file = RegisteredMasterFile::new(&registered)?;
    let crtc = file.crtc()?.to_owned_ref();
    let connector = file.connector()?;
    let object = shmem::Object::<gem::Object>::new(
        &registered,
        640 * 480 * 4,
        Default::default(),
        Default::default(),
    )?;
    let framebuffer = Framebuffer::from_objects(
        &registered,
        &FramebufferLayout {
            width: 640,
            height: 480,
            format: drm::fourcc::XRGB8888,
            modifier: None,
            interlaced: false,
            planes: &[FramebufferPlane {
                object: &object,
                pitch: 2560,
                offset: 0,
            }],
        },
    )?;
    let mode = DisplayMode::from_timings(ModeTimings {
        clock_khz: 25175,
        hdisplay: 640,
        hsync_start: 656,
        hsync_end: 752,
        htotal: 800,
        vdisplay: 480,
        vsync_start: 490,
        vsync_end: 492,
        vtotal: 525,
        flags: ModeFlags::NHSYNC | ModeFlags::NVSYNC,
    })?;
    let connectors = [&*connector];
    let scanout = CrtcScanout {
        mode: &mode,
        framebuffer: &framebuffer,
        connectors: &connectors,
        position: (0, 0),
    };
    registered.atomic_update(|state| state.set_crtc_config(crtc.crtc(), Some(&scanout)))?;
    f(&registered, crtc.crtc(), &connector, &scanout, file)
}

fn owner(
    file: &RegisteredMasterFile<'_, Driver>,
    crtc: &Crtc<display::Crtc>,
    connector: &Connector<display::Connector>,
) -> Result<Owner> {
    let permission = {
        let snapshot = file.file().master_snapshot().ok_or(EINVAL)?;
        let guard = snapshot.master().lock_current().ok_or(EACCES)?;
        Permission::new(&guard, crtc, connector)?
    };
    Owner::new(permission)
}

#[kunit_tests(rust_castkms_registered_renderer_control)]
mod cases {
    use super::*;

    #[test]
    fn fresh_host_allocation_never_reopens_the_retired_worker() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Candidate::begin(owner.access())?;
            let host = device.host.clone();
            let layout = crate::host_compositor::layout::Layout::new(640, 480)?;
            let old = host.configure(device, layout)?;
            old.request()?;
            host.with_change(|change| {
                candidate.with_activation_control(device, |_, _| change.disable())
            })?;
            host.with_change(|change| {
                candidate.with_activation_control(device, |_, _| change.enable())
            })?;
            check(old.request() == Err(ENODEV))?;
            check(matches!(host.current(), Err(EAGAIN)))?;
            let fresh = host.configure(device, layout)?;
            let request = fresh.request_outcome()?;
            check(matches!(
                request.wait()?,
                crate::host_compositor::worker::Outcome::Image(_)
            ))?;
            host.with_change(|change| change.enable())?;
            check(fresh.last_image().is_some())?;
            check(old.request() == Err(ENODEV))
        })
    }

    #[test]
    fn host_cutoff_runs_inside_control_and_cleanup_runs_afterward() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Candidate::begin(owner.access())?;
            let host = device.host.clone();
            let handle = host.configure(
                device,
                crate::host_compositor::layout::Layout::new(640, 480)?,
            )?;
            handle.request()?;
            host.with_change(|change| {
                candidate.with_activation_control(device, |_, _| {
                    change.disable()?;
                    check(handle.request() == Err(ENODEV))?;
                    check(matches!(host.current(), Err(EOPNOTSUPP)))
                })
            })?;
            check(handle.take_outcome().is_none())?;
            check(matches!(
                host.configure(
                    device,
                    crate::host_compositor::layout::Layout::new(640, 480)?,
                ),
                Err(EOPNOTSUPP)
            ))?;
            check(device.execution.describe().profile == crate::execution::Profile::HostV1)
        })
    }

    #[test]
    fn rejected_display_control_preserves_host_worker_admission() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Candidate::begin(owner.access())?;
            let host = device.host.clone();
            let handle = host.configure(
                device,
                crate::host_compositor::layout::Layout::new(640, 480)?,
            )?;
            owner.revoke();
            check(
                host.with_change(|change| {
                    candidate.with_activation_control(device, |_, _| change.disable())
                }) == Err(EKEYREVOKED),
            )?;
            let _request = handle.request_outcome()?;
            check(host.current().is_ok())
        })
    }

    #[test]
    fn installed_control_survives_content_updates_and_observes_revocation() -> Result {
        with_display(|device, crtc, connector, scanout, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Candidate::begin(owner.access())?;
            let mut calls = 0;
            for _ in 0..16 {
                device.atomic_update(|state| state.set_crtc_config(crtc, Some(scanout)))?;
                candidate.with_activation_control(device, |current, locked| {
                    calls += 1;
                    current.check_source(locked.preparation_source(crtc)?.ok_or(EINVAL)?)?;
                    check(current.configuration().dimensions() == [640, 480])
                })?;
            }
            owner.revoke();
            check(
                candidate.with_activation_control(device, |_, _| {
                    calls += 1;
                    Ok(())
                }) == Err(EKEYREVOKED),
            )?;
            check(calls == 16)
        })
    }

    #[test]
    fn cancellation_and_master_close_stop_registered_control() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Candidate::begin(owner.access())?;
            candidate.cancel();
            check(candidate.with_activation_control(device, |_, _| Ok(())) == Err(ECANCELED))?;
            drop(candidate);
            let replacement = Candidate::begin(owner.access())?;
            replacement.with_activation_control(device, |_, _| Ok(()))?;
            drop(file);
            check(replacement.with_activation_control(device, |_, _| Ok(())) == Err(EACCES))
        })
    }

    #[test]
    fn installed_control_rejects_publication_from_a_preceding_commit() -> Result {
        with_display(|device, crtc, connector, scanout, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Candidate::begin(owner.access())?;
            let output = &device.output;
            let save = || {
                output
                    .with_accepted(|accepted| {
                        accepted.map(|accepted| {
                            (
                                kernel::sync::aref::ARef::from(accepted.source),
                                accepted.scene.cloned(),
                                accepted.configuration.clone(),
                            )
                        })
                    })
                    .ok_or(EINVAL)
            };
            let first = save()?;
            device.atomic_update(|state| state.set_crtc_config(crtc, Some(scanout)))?;
            let latest = save()?;
            // Replay the preceding publication to exercise the mismatch against real accepted
            // CRTC state. This is not an asynchronous-tail timing test, and no pixel reader runs.
            output.publish_with_configuration(
                first.0,
                output::SceneUpdate::Replace(first.1),
                first.2,
            );
            let restore = kernel::types::ScopeGuard::new(|| {
                output.publish_with_configuration(
                    latest.0,
                    output::SceneUpdate::Replace(latest.1),
                    latest.2,
                );
            });
            let mut calls = 0;
            let rejected = candidate.with_activation_control(device, |_, _| {
                calls += 1;
                Ok(())
            });
            drop(restore);
            check(rejected == Err(EAGAIN))?;
            check(calls == 0)?;
            candidate.with_activation_control(device, |_, _| {
                calls += 1;
                Ok(())
            })?;
            check(calls == 1)
        })
    }
}
