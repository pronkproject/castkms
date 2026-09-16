// SPDX-License-Identifier: GPL-2.0-only

//! Shared-device authority and lifetime isolation across the full output topology.

use super::{
    delegated_authority::grant,
    delegated_requests::wait,
    private_images::{activate, buffer},
    *,
};
use crate::{
    capture::provider::{delegated_destination::Image, delegated_request::Status, Grantor},
    renderer::{job::Completion, render_job::Rendered},
    renderer_startup::Active,
};
use kernel::{
    dma_fence::testing::ManualFence,
    drm::{fourcc, gem::ExportAccess},
};

struct Output {
    _owner: Owner,
    renderer: Arc<Candidate>,
    active: Active,
    grantor: Option<Grantor>,
    rendered: Arc<Rendered>,
    destination: Arc<Image>,
}

#[kunit_tests(rust_castkms_delegated_outputs)]
mod cases {
    use super::*;

    #[test]
    fn eight_outputs_keep_capture_authority_and_retirement_independent() -> Result {
        let display = CastKms::new_outputs(c"castkms-eight-delegated-outputs", 8)?;
        let registered = display._display.registration_guard().ok_or(ENODEV)?;
        let file = RegisteredMasterFile::new(&registered)?;
        check(file.crtc_at(8).err() == Some(EINVAL))?;
        check(file.connector_at(8).err() == Some(EINVAL))?;
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
        let mut crtcs = KVec::new();
        let mut connectors = KVec::new();
        let mut frames = KVec::new();
        for index in 0..8 {
            crtcs.push(file.crtc_at(index)?.to_owned_ref(), GFP_KERNEL)?;
            connectors.push(file.connector_at(index)?, GFP_KERNEL)?;
            let object = shmem::Object::<gem::Object>::new(
                &registered,
                640 * 480 * 4,
                Default::default(),
                Default::default(),
            )?;
            frames.push(
                Framebuffer::from_objects(
                    &registered,
                    &FramebufferLayout {
                        width: 640,
                        height: 480,
                        format: fourcc::XRGB8888,
                        modifier: None,
                        interlaced: false,
                        planes: &[FramebufferPlane {
                            object: &object,
                            pitch: 2560,
                            offset: 0,
                        }],
                    },
                )?,
                GFP_KERNEL,
            )?;
        }
        registered.atomic_update(|mut transaction| {
            for index in 0..8 {
                transaction.as_mut().set_crtc_config(
                    crtcs[index].crtc(),
                    Some(&CrtcScanout {
                        mode: &mode,
                        framebuffer: &frames[index],
                        connectors: &[&*connectors[index]],
                        position: (0, 0),
                    }),
                )?;
            }
            Ok(())
        })?;
        let mut outputs = KVec::new();
        for index in 0..8 {
            let crtc = crtcs[index].crtc();
            let owner = owner(&file, crtc, &connectors[index])?;
            let renderer = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, execution) = activate(&renderer, &registered, crtc)?;
            let grantor = grant(&file, crtc, &connectors[index])?;
            let private = renderer.register_private_image(
                &active,
                [640, 480],
                &[buffer(&registered, ExportAccess::ReadWrite)?],
            )?;
            let rendered = Arc::new(
                renderer
                    .claim_render(&active, execution, None, private.prepare(1)?)?
                    .release(Completion::Cpu)
                    .ok_or(EINVAL)?,
                GFP_KERNEL,
            )?;
            let destination = grantor
                .capture()
                .describe_delegated()?
                .register_destination(
                    &buffer(&registered, ExportAccess::ReadWrite)?,
                    fourcc::XRGB8888,
                    0,
                    2560,
                    0,
                )?;
            outputs.push(
                Output {
                    _owner: owner,
                    renderer,
                    active,
                    grantor: Some(grantor),
                    rendered,
                    destination,
                },
                GFP_KERNEL,
            )?;
        }
        // Same device and same master do not make output identities interchangeable.
        let wrong_output = outputs[1].destination.request(1, None)?;
        check(
            wrong_output
                .try_claim(
                    &outputs[0].renderer,
                    &outputs[0].active,
                    &outputs[0].rendered,
                )
                .err()
                == Some(EACCES),
        )?;
        check(wrong_output.status() == Status::Complete(Err(EACCES)))?;
        let alias = outputs[0].rendered.image().buffers()[0].clone();
        check(
            outputs[7]
                .renderer
                .register_private_image(&outputs[7].active, [640, 480], &[alias])
                .err()
                == Some(EEXIST),
        )?;

        let pending = outputs[0].destination.request(2, None)?;
        let claim = pending
            .try_claim(
                &outputs[0].renderer,
                &outputs[0].active,
                &outputs[0].rendered,
            )?
            .ok_or(EINVAL)?;
        let mut native = ManualFence::new()?;
        claim.release(Completion::Submitted(native.fence()));
        drop(outputs[0].grantor.take());
        for output in &outputs[1..] {
            let request = output.destination.request(2, None)?;
            let claim = request
                .try_claim(&output.renderer, &output.active, &output.rendered)?
                .ok_or(EINVAL)?;
            claim.release(Completion::Cpu);
            check(request.status() == Status::Complete(Ok(())))?;
            check(pending.status() == Status::Pending)?;
        }
        // Content updates on the other CRTCs do not wait for the first output's recipient.
        registered.atomic_update(|mut transaction| {
            for index in 1..8 {
                transaction.as_mut().set_crtc_config(
                    crtcs[index].crtc(),
                    Some(&CrtcScanout {
                        mode: &mode,
                        framebuffer: &frames[index],
                        connectors: &[&*connectors[index]],
                        position: (0, 0),
                    }),
                )?;
            }
            Ok(())
        })?;
        check(pending.status() == Status::Pending)?;
        native.complete(Ok(()))?;
        wait(&pending, Status::Complete(Err(EKEYREVOKED)))?;
        for output in &outputs[1..] {
            let request = output.destination.request(3, None)?;
            let claim = request
                .try_claim(&output.renderer, &output.active, &output.rendered)?
                .ok_or(EINVAL)?;
            claim.release(Completion::Cpu);
            check(request.status() == Status::Complete(Ok(())))?;
        }
        Ok(())
    }
}
