// SPDX-License-Identifier: GPL-2.0-only

//! Kernel-issued host capture with real display authority and independent result storage.

use super::*;
use crate::{
    capture::{
        permission::Permission,
        provider::Grantor, //
    },
    host_compositor::{
        compose,
        layout::Layout,
        pool::Pool, //
    }, //
};
use kernel::{
    drm::{
        capture::Status,
        kms::testing::MasterFile,
        preparation::Source, //
    },
    io::{
        io_project,
        Io, //
    },
    sync::{
        aref::ARef,
        Arc, //
    }, //
};

fn grant(fixture: &Fixture, file: &MasterFile<'_, Driver>) -> Result<Grantor> {
    let permission = {
        let snapshot = file.file().master_snapshot().ok_or(EINVAL)?;
        let guard = snapshot.master().lock_current().ok_or(EACCES)?;
        Permission::new(&guard, fixture.drm.crtc()?, fixture.drm.connector()?)?
    };
    Grantor::new(permission)
}

fn selected_framebuffer(
    fixture: &Fixture,
    file: &MasterFile<'_, Driver>,
) -> Result<FramebufferRef<Driver>> {
    let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
        file.file().master_snapshot(),
    ))?;
    fixture.select(&fb, false, 0)?;
    Ok(fb)
}

fn pool(fixture: &Fixture) -> Result<Arc<Pool>> {
    Pool::new(
        fixture.drm.device(),
        &fixture.host_budget,
        Layout::new(640, 480)?,
    )
}

#[kunit_tests(rust_castkms_capture_provider)]
mod cases {
    use super::*;

    #[test]
    fn explicit_close_stops_retained_stream_owners() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = selected_framebuffer(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        let stream = Arc::new(capture.stream(2)?, GFP_KERNEL)?;
        let retained = stream.clone();
        let sibling = capture.stream(1)?;
        let complete = stream.queue()?;
        let queued = stream.queue()?;
        let image =
            compose::current(&fixture.drm.device().output, &pool(&fixture)?)?.ok_or(EINVAL)?;
        stream.deliver(&image)?;
        stream.close();
        stream.close();
        drop(stream);
        check(complete.status() == Err(ENOENT))?;
        check(queued.status() == Err(ENOENT))?;
        check(matches!(retained.queue(), Err(EKEYREVOKED)))?;
        check(retained.deliver(&image) == Err(ENOENT))?;
        let sibling_request = sibling.queue()?;
        sibling.deliver(&image)?;
        check(sibling_request.status()? == Status::Complete(Ok(())))
    }

    #[test]
    fn selected_delivery_does_not_consume_other_demand() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = selected_framebuffer(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let stream = grantor.capture().stream(2)?;
        let first = stream.queue()?;
        let second = stream.queue()?;
        let image =
            compose::current(&fixture.drm.device().output, &pool(&fixture)?)?.ok_or(EINVAL)?;
        second.deliver(&stream, &image)?;
        check(first.status()? == Status::Pending)?;
        check(second.status()? == Status::Complete(Ok(())))?;
        check(second.deliver(&stream, &image) == Err(EALREADY))?;
        first.cancel()?;
        check(first.deliver(&stream, &image) == Err(EALREADY))?;
        Ok(())
    }

    #[test]
    fn selected_delivery_rejects_foreign_stream_and_revoked_grant() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = selected_framebuffer(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        let first = capture.stream(1)?;
        let second = capture.stream(1)?;
        let first_request = first.queue()?;
        let second_request = second.queue()?;
        let image =
            compose::current(&fixture.drm.device().output, &pool(&fixture)?)?.ok_or(EINVAL)?;
        check(first_request.deliver(&second, &image) == Err(EINVAL))?;
        check(first_request.status()? == Status::Pending)?;
        check(second_request.status()? == Status::Pending)?;
        drop(grantor);
        check(first_request.deliver(&first, &image) == Err(EKEYREVOKED))?;
        Ok(())
    }

    #[test]
    fn authorized_pixels_arrive_without_retaining_scanout_reads() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let fb = selected_framebuffer(&fixture, &file)?;
        {
            let mapping = fb.vmap::<gem::Object>()?;
            io_project!(mapping.view(), [try: 0..2560]).copy_from_slice(&[0x35; 2560]);
        }
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        let stream = capture.stream(2)?;
        let request = stream.queue()?;
        let output = &fixture.drm.device().output;
        let source: ARef<Source> = output
            .inspect_accepted(|accepted| accepted.map(|(source, _)| source.into()))
            .ok_or(EINVAL)?;
        let images = pool(&fixture)?;
        let image = compose::current(output, &images)?.ok_or(EINVAL)?;
        let admission = source.hold_admission()?;
        check(admission.prepared()?.is_some())?;
        stream.deliver(&image)?;
        drop(image);
        images.close();
        check(admission.prepared()?.is_some())?;
        check(request.status()? == Status::Complete(Ok(())))?;
        check(request.wait()? == Ok(()))?;
        let mut pixels = KVVec::new();
        pixels.resize(Layout::new(640, 480)?.pixel_bytes(), 0xa7, GFP_KERNEL)?;
        check(request.copy_result(&mut pixels)? == pixels.len())?;
        check(
            pixels[..2560]
                .chunks_exact(4)
                .all(|pixel| pixel == [0x35, 0x35, 0x35, 0xff]),
        )?;
        Ok(())
    }

    #[test]
    fn grantor_close_revokes_demand_but_keeps_completed_authorized_results() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = selected_framebuffer(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        let stream = capture.stream(2)?;
        let completed = stream.queue()?;
        let pending = stream.queue()?;
        let image =
            compose::current(&fixture.drm.device().output, &pool(&fixture)?)?.ok_or(EINVAL)?;
        stream.deliver(&image)?;
        drop(grantor);
        check(matches!(capture.stream(1), Err(EKEYREVOKED)))?;
        check(matches!(stream.queue(), Err(EKEYREVOKED)))?;
        check(pending.status()? == Status::Complete(Err(EKEYREVOKED)))?;
        check(completed.status()? == Status::Complete(Ok(())))?;
        let mut pixels = KVVec::new();
        pixels.resize(image.layout().pixel_bytes(), 0xa7, GFP_KERNEL)?;
        check(completed.copy_result(&mut pixels)? == pixels.len())?;
        check(pixels.chunks_exact(4).all(|pixel| pixel == [0, 0, 0, 0xff]))?;
        Ok(())
    }

    #[test]
    fn closing_one_stream_does_not_revoke_its_sibling() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = selected_framebuffer(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        let first = capture.stream(1)?;
        let second = capture.stream(1)?;
        let first_request = first.queue()?;
        let second_request = second.queue()?;
        drop(capture);
        drop(first);
        check(first_request.status() == Err(ENOENT))?;
        check(second_request.status()? == Status::Pending)?;
        let image =
            compose::current(&fixture.drm.device().output, &pool(&fixture)?)?.ok_or(EINVAL)?;
        second.deliver(&image)?;
        check(second_request.status()? == Status::Complete(Ok(())))?;
        drop(grantor);
        drop(second);
        check(second_request.status() == Err(ENOENT))?;
        Ok(())
    }

    #[test]
    fn mode_replacement_requires_new_streams_and_matching_images() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let fb = selected_framebuffer(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        let old = capture.stream(1)?;
        let request = old.queue()?;
        let images = pool(&fixture)?;
        let old_image = compose::current(&fixture.drm.device().output, &images)?.ok_or(EINVAL)?;
        fixture.drm.update(|mut transaction| {
            transaction
                .as_mut()
                .set_crtc_config(fixture.drm.crtc()?, None)
        })?;
        check(request.status()? == Status::Complete(Err(EKEYREVOKED)))?;
        check(matches!(capture.stream(1), Err(ENODEV)))?;
        fixture.select(&fb, false, 0)?;
        let current = capture.stream(1)?;
        let current_request = current.queue()?;
        check(current.deliver(&old_image) == Err(EACCES))?;
        check(current_request.status()? == Status::Pending)?;
        let current_image =
            compose::current(&fixture.drm.device().output, &images)?.ok_or(EINVAL)?;
        check(old.deliver(&current_image) == Err(ESTALE))?;
        current.deliver(&current_image)?;
        check(current_request.status()? == Status::Complete(Ok(())))?;
        Ok(())
    }

    #[test]
    fn master_close_ends_old_streams_without_authorizing_the_replacement() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let old_file = fixture.drm.master_file()?;
        let _old_fb = selected_framebuffer(&fixture, &old_file)?;
        let old_grantor = grant(&fixture, &old_file)?;
        let old_capture = old_grantor.capture();
        let old_stream = old_capture.stream(1)?;
        let request = old_stream.queue()?;
        drop(old_file);
        check(request.status()? == Status::Complete(Err(EKEYREVOKED)))?;
        check(matches!(old_capture.stream(1), Err(EACCES)))?;
        let file = fixture.drm.master_file()?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        check(matches!(capture.stream(1), Err(EACCES)))?;
        let _new_fb = selected_framebuffer(&fixture, &file)?;
        let _new_stream = capture.stream(1)?;
        check(matches!(old_stream.queue(), Err(EKEYREVOKED)))?;
        Ok(())
    }

    #[test]
    fn request_references_keep_closed_stream_reservations() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = selected_framebuffer(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        let mut requests = KVec::new();
        for _ in 0..16 {
            let stream = capture.stream(1)?;
            requests.push(stream.queue()?, GFP_KERNEL)?;
        }
        check(matches!(capture.stream(1), Err(EBUSY)))?;
        drop(requests.pop());
        let replacement = capture.stream(1)?;
        check(matches!(capture.stream(1), Err(EBUSY)))?;
        drop(replacement);
        drop(requests);
        let _fresh = capture.stream(8)?;
        Ok(())
    }

    #[test]
    fn cancellation_keeps_result_credit_until_the_request_is_dropped() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = selected_framebuffer(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let stream = grantor.capture().stream(1)?;
        let request = stream.queue()?;
        request.cancel()?;
        check(request.status()? == Status::Complete(Err(ECANCELED)))?;
        check(matches!(stream.queue(), Err(EAGAIN)))?;
        drop(request);
        let _replacement = stream.queue()?;
        Ok(())
    }

    #[test]
    fn device_shutdown_closes_admission_even_with_retained_grants() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = selected_framebuffer(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        let stream = capture.stream(1)?;
        let request = stream.queue()?;
        fixture.state.close();
        check(request.status()? == Status::Complete(Err(EKEYREVOKED)))?;
        check(matches!(stream.queue(), Err(EKEYREVOKED)))?;
        check(matches!(capture.stream(1), Err(EKEYREVOKED)))?;
        Ok(())
    }

    #[test]
    fn consumer_backlog_is_deeper_than_the_private_compositor_pool() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let fb = selected_framebuffer(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let stream = grantor.capture().stream(6)?;
        let images = pool(&fixture)?;
        let output = &fixture.drm.device().output;
        let mut requests = KVec::new();
        for frame in 1..=6u8 {
            {
                let mapping = fb.vmap::<gem::Object>()?;
                io_project!(mapping.view(), [try: 0..2560]).copy_from_slice(&[frame; 2560]);
            }
            fixture.select(&fb, false, 0)?;
            let request = stream.queue()?;
            let image = compose::current(output, &images)?.ok_or(EINVAL)?;
            let source: ARef<Source> = output
                .inspect_accepted(|accepted| accepted.map(|(source, _)| source.into()))
                .ok_or(EINVAL)?;
            let admission = source.hold_admission()?;
            check(admission.prepared()?.is_some())?;
            stream.deliver(&image)?;
            requests.push(request, GFP_KERNEL)?;
        }
        check(matches!(stream.queue(), Err(EAGAIN)))?;
        images.close();
        let mut pixels = KVVec::new();
        pixels.resize(Layout::new(640, 480)?.pixel_bytes(), 0xa7, GFP_KERNEL)?;
        for (index, request) in requests.iter().enumerate() {
            check(request.copy_result(&mut pixels)? == pixels.len())?;
            let frame = index as u8 + 1;
            check(
                pixels[..2560]
                    .chunks_exact(4)
                    .all(|pixel| pixel == [frame, frame, frame, 0xff]),
            )?;
        }
        Ok(())
    }

    #[test]
    fn registration_failure_releases_reserved_stream_capacity() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = selected_framebuffer(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        // Keep accepted ownership live while closing the final registration boundary.
        fixture.drm.device().capture_streams.close();
        for _ in 0..32 {
            check(matches!(capture.stream(1), Err(ENODEV)))?;
        }
        let mut charges = KVec::new();
        for _ in 0..16 {
            charges.push(
                fixture
                    .drm
                    .device()
                    .capture_budget
                    .reserve(Layout::new(1, 1)?, 1)?,
                GFP_KERNEL,
            )?;
        }
        Ok(())
    }
}
