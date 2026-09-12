// SPDX-License-Identifier: GPL-2.0-only

//! Independent capture storage; the fixture owns every source and recipient.

use super::*;
use crate::host_compositor::{
    layout::Layout,
    worker::Outcome, //
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

#[kunit_tests(rust_castkms_host_requests)]
mod cases {
    use super::*;

    #[test]
    fn pending_capture_results_do_not_exhaust_private_images() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let layout = Layout::new(640, 480)?;
        let configuration = &fixture.drm.device().host;
        let handle = configuration.configure(fixture.drm.device(), layout)?;
        let stream = Stream::new(6, layout.pixel_bytes())?;
        let mut requests = KVVec::new();
        let mut jobs = KVVec::new();
        let mut sources = KVVec::new();
        for frame in 0..6u8 {
            {
                let mapping = fb.vmap::<gem::Object>()?;
                io_project!(mapping.view(), [try: 0..2560]).copy_from_slice(&[frame + 1; 2560]);
            }
            fixture.select(&fb, false, 0)?;
            let source: ARef<Source> = fixture
                .drm
                .device()
                .output
                .inspect_accepted(|accepted| accepted.map(|(source, _)| source.into()))
                .ok_or(EINVAL)?;
            let request = stream.queue()?;
            let mut job = stream.claim()?;
            handle.request()?;
            let Outcome::Image(image) = handle.wait_for_outcome()? else {
                return Err(EINVAL);
            };
            image.copy_pixels(job.data_mut())?;
            drop(image);
            check(request.status()? == Status::Pending)?;
            requests.push(request, GFP_KERNEL)?;
            jobs.push(job, GFP_KERNEL)?;
            sources.push(source, GFP_KERNEL)?;
        }

        configuration.stop_worker()?;
        fixture.state.close();
        check(handle.request() == Err(ENODEV))?;
        for source in &sources {
            check(source.prepared()?.is_some())?;
        }
        for request in &requests {
            check(request.status()? == Status::Pending)?;
        }
        requests[0].cancel()?;
        for (frame, mut job) in jobs.into_iter().enumerate() {
            check(
                job.data_mut()[..2560]
                    .iter()
                    .all(|pixel| *pixel == frame as u8 + 1),
            )?;
            job.complete(Ok(()));
        }
        let mut pixels = KVVec::new();
        pixels.resize(layout.pixel_bytes(), 0xff, GFP_KERNEL)?;
        for (frame, request) in requests.iter().enumerate() {
            if frame == 0 {
                check(request.status()? == Status::Complete(Err(ECANCELED)))?;
                check(request.copy_result(&mut pixels) == Err(ECANCELED))?;
                check(pixels.iter().all(|pixel| *pixel == 0xff))?;
                continue;
            }
            check(request.status()? == Status::Complete(Ok(())))?;
            check(request.copy_result(&mut pixels)? == layout.pixel_bytes())?;
            check(pixels[..2560].iter().all(|pixel| *pixel == frame as u8 + 1))?;
            check(pixels[2560..].iter().all(|pixel| *pixel == 0))?;
        }
        Ok(())
    }

    #[test]
    fn revoking_a_capture_stream_does_not_close_the_private_worker() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let layout = Layout::new(640, 480)?;
        let handle = fixture
            .drm
            .device()
            .host
            .configure(fixture.drm.device(), layout)?;
        let stream = Stream::new(1, layout.pixel_bytes())?;
        let request = stream.queue()?;
        let mut job = stream.claim()?;
        handle.request()?;
        let Outcome::Image(image) = handle.wait_for_outcome()? else {
            return Err(EINVAL);
        };
        image.copy_pixels(job.data_mut())?;
        drop(image);
        stream.revoke();

        // The revoked job still owns independent writable storage until completion.
        job.data_mut().fill(0x71);
        for _ in 0..4 {
            fixture.select(&fb, false, 0)?;
            handle.request()?;
            check(matches!(handle.wait_for_outcome()?, Outcome::Image(_)))?;
        }
        fixture.state.close();
        job.complete(Ok(()));
        check(request.status()? == Status::Complete(Err(EKEYREVOKED)))?;
        let mut pixels = KVVec::new();
        pixels.resize(layout.pixel_bytes(), 0xa7, GFP_KERNEL)?;
        check(request.copy_result(&mut pixels) == Err(EKEYREVOKED))?;
        check(pixels.iter().all(|pixel| *pixel == 0xa7))?;
        Ok(())
    }
}
