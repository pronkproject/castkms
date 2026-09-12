// SPDX-License-Identifier: GPL-2.0-only

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

#[kunit_tests(rust_castkms_host_results)]
mod cases {
    use super::*;

    #[test]
    fn a_capture_job_keeps_pixels_after_private_storage_is_released() -> Result {
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
        let configuration = &fixture.drm.device().host;
        let handle = configuration.configure(fixture.drm.device(), Layout::new(640, 480)?)?;
        handle.request()?;
        handle.flush_for_test();
        let Some(Outcome::Image(image)) = handle.take_outcome() else {
            return Err(EINVAL);
        };
        check(source.hold_admission()?.prepared()?.is_some())?;

        // The fixture owns both the source and the recipient; no pixels leave the test.
        let size = image.layout().pixel_bytes();
        let stream = Stream::new(1, size)?;
        let request = stream.queue()?;
        let mut job = stream.claim()?;
        image.copy_pixels(job.data_mut())?;
        drop(image);
        fixture.state.close();
        drop(fb);
        check(source.prepared()?.is_some())?;
        check(request.status()? == Status::Pending)?;
        job.complete(Ok(()));

        let mut pixels = KVVec::new();
        pixels.resize(size, 0xff, GFP_KERNEL)?;
        check(request.status()? == Status::Complete(Ok(())))?;
        check(request.copy_result(&mut pixels)? == size)?;
        check(pixels[..2560].iter().all(|pixel| *pixel == 0x35))?;
        check(pixels[2560..1226240].iter().all(|pixel| *pixel == 0))?;
        check(pixels[1226240..].iter().all(|pixel| *pixel == 0x71))?;
        Ok(())
    }

    #[test]
    fn a_mismatched_capture_job_is_not_partially_filled() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let configuration = &fixture.drm.device().host;
        let handle = configuration.configure(fixture.drm.device(), Layout::new(640, 480)?)?;
        handle.request()?;
        handle.flush_for_test();
        let Some(Outcome::Image(image)) = handle.take_outcome() else {
            return Err(EINVAL);
        };
        let stream = Stream::new(1, 24)?;
        let request = stream.queue()?;
        let mut job = stream.claim()?;
        job.data_mut().fill(0xa7);
        check(image.copy_pixels(job.data_mut()) == Err(EINVAL))?;
        check(job.data_mut() == &[0xa7; 24])?;
        drop(job);
        check(request.status()? == Status::Complete(Err(ECANCELED)))?;
        Ok(())
    }

    #[test]
    fn completed_layout_survives_worker_replacement_and_device_shutdown() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let configuration = &fixture.drm.device().host;
        let layout = Layout::new(640, 480)?;
        let handle = configuration.configure(fixture.drm.device(), layout)?;
        handle.request()?;
        handle.flush_for_test();
        let Some(Outcome::Image(image)) = handle.take_outcome() else {
            return Err(EINVAL);
        };
        check(image.layout() == layout)?;
        let _replacement = configuration.configure(fixture.drm.device(), Layout::new(3, 2)?)?;
        check(handle.request() == Err(ENODEV))?;
        check(image.layout().dimensions() == (640, 480))?;
        check(image.layout().pitch() == 2560)?;
        fixture.state.close();
        check(image.layout() == layout)?;
        let mut row = [0xff; 2560];
        image.read_row(479, &mut row)?;
        check(row == [0; 2560])?;
        Ok(())
    }
}
