// SPDX-License-Identifier: GPL-2.0-only

//! Historical frame descriptions survive out-of-order delivery without retaining private images.

use super::*;
use kernel::time::Delta;

#[kunit_tests(rust_castkms_capture_frames)]
mod cases {
    use super::*;

    #[test]
    fn reversed_frames_retain_their_original_pixels_and_metadata() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let fb = selected_framebuffer(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let stream = grantor.capture().stream(2)?;
        let first = stream.queue()?;
        let second = stream.queue()?;
        let pool = pool(&fixture)?;
        let output = &fixture.drm.device().output;
        let old_image = compose::current(output, &pool)?.ok_or(EINVAL)?;
        let old_serial = old_image.content_serial();
        let old_time = old_image.completed_at();
        {
            let mapping = fb.vmap::<gem::Object>()?;
            io_project!(mapping.view(), [try: 0..2560]).copy_from_slice(&[0x71; 2560]);
        }
        fixture.select(&fb, false, 0)?;
        let new_image = compose::current(output, &pool)?.ok_or(EINVAL)?;
        let new_serial = new_image.content_serial();
        check(new_serial != old_serial)?;
        let second = second.deliver_frame(&stream, &new_image)?;
        let first = first.deliver_frame(&stream, &old_image)?;
        check(first.metadata().content_serial() == old_serial)?;
        check(second.metadata().content_serial() == new_serial)?;
        check(first.metadata().completed_at() - old_time == Delta::ZERO)?;
        check(second.metadata().completed_at() - new_image.completed_at() == Delta::ZERO)?;
        check(first.metadata().configuration() == old_image.configuration().ok_or(EINVAL)?)?;
        check(first.metadata().output_identity() == output.identity())?;
        check(first.metadata().layout() == old_image.layout())?;
        drop(old_image);
        drop(new_image);
        let _first_slot = pool.reserve()?;
        let _second_slot = pool.reserve()?;
        let mut pixels = KVVec::new();
        pixels.resize(first.metadata().layout().pixel_bytes(), 0xa7, GFP_KERNEL)?;
        first.request().copy_result(&mut pixels)?;
        check(
            pixels[..2560]
                .chunks_exact(4)
                .all(|pixel| pixel == [0, 0, 0, 0xff]),
        )?;
        second.request().copy_result(&mut pixels)?;
        check(
            pixels[..2560]
                .chunks_exact(4)
                .all(|pixel| pixel == [0x71, 0x71, 0x71, 0xff]),
        )?;
        let metadata = first.metadata().clone();
        drop(grantor);
        check(first.request().status()? == Status::Complete(Ok(())))?;
        drop(stream);
        check(first.request().status() == Err(ENOENT))?;
        drop(first);
        check(metadata.content_serial() == old_serial)?;
        check(metadata.completed_at() - old_time == Delta::ZERO)
    }

    #[test]
    fn blank_frame_does_not_claim_a_framebuffer_revision() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = selected_framebuffer(&fixture, &file)?;
        fixture
            .drm
            .update(|transaction| transaction.disable_plane(fixture.drm.plane()?))?;
        let grantor = grant(&fixture, &file)?;
        let stream = grantor.capture().stream(1)?;
        let image =
            compose::current(&fixture.drm.device().output, &pool(&fixture)?)?.ok_or(EINVAL)?;
        let frame = stream.queue()?.deliver_frame(&stream, &image)?;
        check(frame.metadata().content_serial().is_none())?;
        check(frame.metadata().configuration().dimensions() == [640, 480])?;
        check(frame.request().status()? == Status::Complete(Ok(())))
    }
}
