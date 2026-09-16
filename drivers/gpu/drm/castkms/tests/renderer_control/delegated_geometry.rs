// SPDX-License-Identifier: GPL-2.0-only

use super::{
    delegated_authority::grant,
    delegated_requests::wait,
    private_images::{activate, buffer},
    *,
};
use crate::{capture::provider::delegated_request::Status, renderer::job::Completion};
use kernel::{
    dma_fence::testing::ManualFence,
    drm::{fourcc, gem::ExportAccess},
};

#[kunit_tests(rust_castkms_delegated_geometry)]
mod cases {
    use super::*;

    #[test]
    fn delegated_output_does_not_inherit_the_host_width_ceiling() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let renderer = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, execution) = activate(&renderer, device, crtc)?;
            check(crate::host_compositor::layout::Layout::new(10000, 2).err() == Some(EINVAL))?;
            let object = shmem::Object::<gem::Object>::new(
                device,
                kernel::page::page_align(80000).ok_or(EOVERFLOW)?,
                Default::default(),
                Default::default(),
            )?;
            let framebuffer = Framebuffer::from_objects(
                device,
                &FramebufferLayout {
                    width: 10000,
                    height: 2,
                    format: fourcc::XRGB8888,
                    modifier: None,
                    interlaced: false,
                    planes: &[FramebufferPlane {
                        object: &object,
                        pitch: 40000,
                        offset: 0,
                    }],
                },
            )?;
            let mode = DisplayMode::from_timings(ModeTimings {
                clock_khz: 3000,
                hdisplay: 10000,
                hsync_start: 10008,
                hsync_end: 10016,
                htotal: 10024,
                vdisplay: 2,
                vsync_start: 3,
                vsync_end: 4,
                vtotal: 5,
                flags: ModeFlags::NHSYNC | ModeFlags::NVSYNC,
            })?;
            device.atomic_update(|transaction| {
                transaction.set_crtc_config(
                    crtc,
                    Some(&CrtcScanout {
                        mode: &mode,
                        framebuffer: &framebuffer,
                        connectors: &[connector],
                        position: (0, 0),
                    }),
                )
            })?;
            let grantor = grant(&file, crtc, connector)?;
            let scope = grantor.capture().describe_delegated()?;
            check(scope.dimensions() == [10000, 2])?;
            let destination = scope.register_destination(
                &buffer(device, ExportAccess::ReadWrite)?,
                fourcc::XRGB8888,
                0,
                40000,
                0,
            )?;
            let private = renderer.register_private_image(
                &active,
                [10000, 2],
                &[buffer(device, ExportAccess::ReadWrite)?],
            )?;
            let rendered = Arc::new(
                renderer
                    .claim_render(&active, execution, None, private.prepare(1)?)?
                    .release(Completion::Cpu)
                    .ok_or(EINVAL)?,
                GFP_KERNEL,
            )?;
            let request = destination.request(1, None)?;
            let claim = request
                .try_claim(&renderer, &active, &rendered)?
                .ok_or(EINVAL)?;
            claim.release(Completion::Cpu);
            check(request.status() == Status::Complete(Ok(())))?;
            check(destination.layout().dimensions == [10000, 2])
        })
    }

    #[test]
    fn equal_geometry_after_reenable_does_not_accept_inflight_old_output() -> Result {
        with_display(|device, crtc, connector, scanout, file| {
            let owner = owner(&file, crtc, connector)?;
            let renderer = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, execution) = activate(&renderer, device, crtc)?;
            let grantor = grant(&file, crtc, connector)?;
            let destination = grantor
                .capture()
                .describe_delegated()?
                .register_destination(
                    &buffer(device, ExportAccess::ReadWrite)?,
                    fourcc::XRGB8888,
                    0,
                    2560,
                    0,
                )?;
            let private = renderer.register_private_image(
                &active,
                [640, 480],
                &[buffer(device, ExportAccess::ReadWrite)?],
            )?;
            let rendered = Arc::new(
                renderer
                    .claim_render(&active, execution, None, private.prepare(1)?)?
                    .release(Completion::Cpu)
                    .ok_or(EINVAL)?,
                GFP_KERNEL,
            )?;
            let request = destination.request(1, None)?;
            let claim = request
                .try_claim(&renderer, &active, &rendered)?
                .ok_or(EINVAL)?;
            let mut native = ManualFence::new()?;
            claim.release(Completion::Submitted(native.fence()));
            device.atomic_update(|transaction| transaction.set_crtc_config(crtc, None))?;
            device.atomic_update(|transaction| transaction.set_crtc_config(crtc, Some(scanout)))?;
            check(request.status() == Status::Pending)?;
            native.complete(Ok(()))?;
            wait(&request, Status::Complete(Err(ESTALE)))?;
            check(request.content_serial().is_none())?;
            check(destination.reserve(2, None).err() == Some(ESTALE))
        })
    }
}
