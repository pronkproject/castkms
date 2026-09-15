// SPDX-License-Identifier: GPL-2.0-only

//! CPU delivery mechanics; the fixture owns every source and recipient.

use super::*;
use crate::{
    capture::host,
    host_compositor::{
        compose,
        layout::Layout,
        pool::Pool, //
    }, //
};
use kernel::{
    drm::{
        capture::{
            Status,
            Stream, //
        },
        preparation::Source, //
    },
    io::{
        io_project,
        Io, //
    },
    sync::aref::ARef, //
};

#[kunit_tests(rust_castkms_host_delivery)]
mod cases {
    use super::*;

    #[test]
    fn unused_bytes_are_defined_before_cpu_delivery() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        {
            let mapping = fb.vmap::<gem::Object>()?;
            io_project!(mapping.view(), [try: 0..2560]).copy_from_slice(&[0x35; 2560]);
            io_project!(mapping.view(), [try: 1226240..1228800]).copy_from_slice(&[0x71; 2560]);
        }
        fixture.select(&fb, false, 0)?;
        let output = &fixture.drm.device().output;
        let source: ARef<Source> = output
            .inspect_accepted(|accepted| accepted.map(|(source, _)| source.into()))
            .ok_or(EINVAL)?;
        let layout = Layout::new(640, 480)?;
        let pool = Pool::new(fixture.drm.device(), &fixture.host_budget, layout)?;
        let image = compose::current(output, &pool)?.ok_or(EINVAL)?;
        check(source.hold_admission()?.prepared()?.is_some())?;
        let stream = Stream::new(1, layout.pixel_bytes())?;
        let request = stream.queue()?;
        let job = stream.claim()?;
        let mut pixels = KVVec::new();
        pixels.resize(layout.pixel_bytes(), 0xa7, GFP_KERNEL)?;
        check(request.copy_result(&mut pixels) == Err(EAGAIN))?;
        check(pixels.iter().all(|byte| *byte == 0xa7))?;

        host::complete(&image, job);
        drop(image);
        pool.close();
        fixture.state.close();
        drop(fb);
        check(source.prepared()?.is_some())?;
        check(request.status()? == Status::Complete(Ok(())))?;
        check(request.copy_result(&mut pixels)? == layout.pixel_bytes())?;
        for (row, data) in pixels.chunks_exact(2560).enumerate() {
            let expected = match row {
                0 => 0x35,
                479 => 0x71,
                _ => 0,
            };
            check(
                data.chunks_exact(4)
                    .all(|pixel| pixel == [expected, expected, expected, 0xff]),
            )?;
        }
        Ok(())
    }

    #[test]
    fn mismatched_size_completes_with_the_copy_error() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let layout = Layout::new(640, 480)?;
        let pool = Pool::new(fixture.drm.device(), &fixture.host_budget, layout)?;
        let image = compose::current(&fixture.drm.device().output, &pool)?.ok_or(EINVAL)?;
        let stream = Stream::new(1, 24)?;
        let request = stream.queue()?;
        let job = stream.claim()?;
        host::complete(&image, job);
        check(request.status()? == Status::Complete(Err(EINVAL)))?;
        let mut pixels = [0xa7; 24];
        check(request.copy_result(&mut pixels) == Err(EINVAL))?;
        check(pixels == [0xa7; 24])?;
        Ok(())
    }

    #[test]
    fn consumer_backlog_does_not_retain_private_images() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let layout = Layout::new(640, 480)?;
        let pool = Pool::new(fixture.drm.device(), &fixture.host_budget, layout)?;
        let stream = Stream::new(6, layout.pixel_bytes())?;
        let mut requests = KVVec::new();
        let mut sources = KVVec::new();
        for frame in 1..=6u8 {
            {
                let mapping = fb.vmap::<gem::Object>()?;
                io_project!(mapping.view(), [try: 0..2560]).copy_from_slice(&[frame; 2560]);
            }
            fixture.select(&fb, false, 0)?;
            let output = &fixture.drm.device().output;
            let source: ARef<Source> = output
                .inspect_accepted(|accepted| accepted.map(|(source, _)| source.into()))
                .ok_or(EINVAL)?;
            let image = compose::current(output, &pool)?.ok_or(EINVAL)?;
            check(source.hold_admission()?.prepared()?.is_some())?;
            let request = stream.queue()?;
            host::complete(&image, stream.claim()?);
            drop(image);
            requests.push(request, GFP_KERNEL)?;
            sources.push(source, GFP_KERNEL)?;
        }
        check(matches!(stream.queue(), Err(EAGAIN)))?;
        pool.close();
        fixture.state.close();
        drop(fb);
        // Retained consumer results must leave the whole private-image budget available.
        let _capacity = fixture.host_budget.reserve(crate::host_compositor::budget::LIMIT)?;
        for source in &sources {
            check(source.prepared()?.is_some())?;
        }
        let mut pixels = KVVec::new();
        pixels.resize(layout.pixel_bytes(), 0xa7, GFP_KERNEL)?;
        for (frame, request) in requests.iter().enumerate() {
            check(request.status()? == Status::Complete(Ok(())))?;
            check(request.copy_result(&mut pixels)? == layout.pixel_bytes())?;
            let value = frame as u8 + 1;
            check(
                pixels[..2560]
                    .chunks_exact(4)
                    .all(|pixel| pixel == [value, value, value, 0xff]),
            )?;
            check(
                pixels[2560..]
                    .chunks_exact(4)
                    .all(|pixel| pixel == [0, 0, 0, 0xff]),
            )?;
        }
        Ok(())
    }

    #[test]
    fn revoked_job_never_delivers_the_completed_image() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let layout = Layout::new(640, 480)?;
        let pool = Pool::new(fixture.drm.device(), &fixture.host_budget, layout)?;
        let image = compose::current(&fixture.drm.device().output, &pool)?.ok_or(EINVAL)?;
        let stream = Stream::new(1, layout.pixel_bytes())?;
        let request = stream.queue()?;
        let job = stream.claim()?;
        stream.revoke();
        host::complete(&image, job);
        check(request.status()? == Status::Complete(Err(EKEYREVOKED)))?;
        let mut pixels = KVVec::new();
        pixels.resize(layout.pixel_bytes(), 0xa7, GFP_KERNEL)?;
        check(request.copy_result(&mut pixels) == Err(EKEYREVOKED))?;
        check(pixels.iter().all(|byte| *byte == 0xa7))?;
        Ok(())
    }
}
