// SPDX-License-Identifier: GPL-2.0-only

//! Useful capture during bounded, concurrent replacement of immutable sources.

use super::*;
use core::sync::atomic::{
    AtomicBool,
    AtomicUsize,
    Ordering, //
};
use kernel::{
    sync::Completion,
    time::{
        delay::fsleep,
        Delta,
        Instant,
        Monotonic, //
    }, //
};

#[pin_data]
struct Reader {
    #[pin]
    work: Work<Self>,
    #[pin]
    started: Completion,
    #[pin]
    stream: Mutex<Stream>,
    #[pin]
    result: Mutex<Option<Result>>,
    stop: AtomicBool,
    frames: AtomicUsize,
}

impl_has_work! {
    impl HasWork<Self> for Reader { self.work }
}

impl Reader {
    fn capture(&self) -> Result {
        let mut pixels = KVVec::new();
        pixels.resize(Layout::new(640, 480)?.pixel_bytes(), 0, GFP_KERNEL)?;
        let started = Instant::<Monotonic>::now();
        while !self.stop.load(Ordering::Acquire) {
            if started.elapsed() >= Delta::from_millis(5000) {
                return Err(ETIMEDOUT);
            }
            match self.stream.lock().capture() {
                Ok(request) => {
                    check(request.copy_result(&mut pixels)? == pixels.len())?;
                    let value = pixels[0];
                    check(value == 0x35 || value == 0x79)?;
                    check(
                        pixels
                            .chunks_exact(4)
                            .all(|pixel| pixel == [value, value, value, 0xff]),
                    )?;
                    self.frames.fetch_add(1, Ordering::Release);
                }
                // An attempt can lose admission while an accepted source is being replaced.
                Err(EAGAIN | EBUSY) => {}
                Err(error) => return Err(error),
            }
            fsleep(Delta::from_millis(1));
        }
        Ok(())
    }
}

impl WorkItem for Reader {
    type Pointer = Arc<Self>;

    fn run(reader: Arc<Self>) {
        reader.started.complete_all();
        let result = reader.capture();
        *reader.result.lock() = Some(result);
    }
}

struct Reading(Arc<Reader>);

impl Reading {
    fn start(stream: Stream) -> Result<Self> {
        let reading = Self(Arc::pin_init(
            pin_init!(Reader {
                work <- new_work!("castkms-capture-progress-test"),
                started <- Completion::new(),
                stream <- kernel::new_mutex!(stream),
                result <- kernel::new_mutex!(None),
                stop: AtomicBool::new(false),
                frames: AtomicUsize::new(0),
            }),
            GFP_KERNEL,
        )?);
        check(workqueue::system_dfl().enqueue(reading.0.clone()).is_ok())?;
        reading.0.started.wait_for_completion();
        Ok(reading)
    }

    fn stop(&self) {
        self.0.stop.store(true, Ordering::Release);
        self.0.work.flush();
    }
}

impl Drop for Reading {
    fn drop(&mut self) {
        self.stop();
    }
}

#[kunit_tests(rust_castkms_capture_progress)]
mod cases {
    use super::*;

    #[test]
    fn capture_keeps_delivering_during_framebuffer_replacement() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let mut buffers = KVec::new();
        for value in [0x35, 0x79] {
            let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
                file.file().master_snapshot(),
            ))?;
            {
                let mapping = fb.vmap::<gem::Object>()?;
                for row in 0..480 {
                    let start = row * 2560;
                    io_project!(mapping.view(), [try: start..start + 2560])
                        .copy_from_slice(&[value; 2560]);
                }
            }
            buffers.push(fb, GFP_KERNEL)?;
        }
        fixture.select(&buffers[0], false, 0)?;
        let grantor = grant(&fixture, &file)?;
        let reader = Reading::start(Stream::new(&grantor.capture(), 1)?)?;
        fixture.select(&buffers[1], false, 0)?;
        let before = reader.0.frames.load(Ordering::Acquire);
        for index in 0..32 {
            fixture.select(&buffers[index % 2], false, 0)?;
        }
        // Only count results delivered before updates stop, not work drained afterwards.
        let after = reader.0.frames.load(Ordering::Acquire);
        reader.stop();
        reader.0.result.lock().take().ok_or(EINVAL)??;
        check(after > before)
    }
}
