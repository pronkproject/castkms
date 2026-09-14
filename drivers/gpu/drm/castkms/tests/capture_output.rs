// SPDX-License-Identifier: GPL-2.0-only

//! Completed host capture copied to independent exported destinations.

mod queued;

use super::*;
use crate::{
    capture::{
        destination::Image,
        host_stream::Stream,
        provider::Grantor, //
    },
    host_compositor::layout::Layout, //
};
use kernel::{
    dma_buf::{
        cpu_access::Write,
        DmaBuf, //
    },
    dma_fence::testing::ManualFence,
    drm::{
        fourcc,
        kms::testing::MasterFile,
        preparation::Source, //
    },
    io::{
        io_project,
        Io,
        SysMem, //
    },
    sync::aref::ARef, //
};

fn select(fixture: &Fixture, file: &MasterFile<'_, Driver>) -> Result<FramebufferRef<Driver>> {
    let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
        file.file().master_snapshot(),
    ))?;
    {
        let map = fb.vmap::<gem::Object>()?;
        io_project!(map.view(), [try: 0..32]).copy_from_slice(&[0x12; 32]);
    }
    fixture.select(&fb, false, 0)?;
    Ok(fb)
}

fn grant(fixture: &Fixture, file: &MasterFile<'_, Driver>) -> Result<Grantor> {
    File::create_capture_grant(file.file(), fixture.drm.crtc()?, fixture.drm.connector()?)
}

fn destination(fixture: &Fixture, layout: Layout) -> Result<Image> {
    let buffer = fixture.drm.export_dumb(656, 482, 32)?;
    let mut initial = KVVec::new();
    initial.resize(buffer.size(), 0x73, GFP_KERNEL)?;
    let mut write = Write::new(&buffer)?;
    write.copy_from_slice(0, &initial)?;
    write.finish()?;
    Image::new(
        buffer,
        layout,
        fourcc::XRGB8888,
        fourcc::FORMAT_MOD_LINEAR,
        2624,
        128,
    )
}

/// Read private test shmem only after synchronous writers have ended.
fn pixels(buffer: &DmaBuf) -> Result<KVVec<u8>> {
    let mut pixels = KVVec::new();
    pixels.resize(buffer.size(), 0, GFP_KERNEL)?;
    // SAFETY: DmaBuf transparently wraps this retained initialized native allocation.
    let raw = core::ptr::from_ref(buffer)
        .cast_mut()
        .cast::<kernel::bindings::dma_buf>();
    let mut map = kernel::bindings::iosys_map::default();
    // SAFETY: Retained private test buffer and exclusive map output, with no locks held.
    kernel::error::to_result(unsafe { kernel::bindings::dma_buf_vmap_unlocked(raw, &mut map) })?;
    let result = (|| {
        if map.is_iomem {
            return Err(EINVAL);
        }
        // SAFETY: The mapping exists before beginning the matching CPU read interval.
        kernel::error::to_result(unsafe {
            kernel::bindings::dma_buf_begin_cpu_access(
                raw,
                kernel::bindings::dma_data_direction_DMA_FROM_DEVICE,
            )
        })?;
        // SAFETY: The map is system memory covering the private export's exact size;
        // all test writes have finished and no native device accesses these pixels.
        let memory = unsafe {
            SysMem::new(core::ptr::slice_from_raw_parts_mut(
                map.__bindgen_anon_1.vaddr.cast::<u8>(),
                pixels.len(),
            ))
        };
        memory.copy_to_slice(&mut pixels);
        // SAFETY: End the successful read while its mapping remains live.
        kernel::error::to_result(unsafe {
            kernel::bindings::dma_buf_end_cpu_access(
                raw,
                kernel::bindings::dma_data_direction_DMA_FROM_DEVICE,
            )
        })
    })();
    // SAFETY: Balance the successful mapping on every path, after any CPU interval ended.
    unsafe { kernel::bindings::dma_buf_vunmap_unlocked(raw, &mut map) };
    result?;
    Ok(pixels)
}

#[kunit_tests(rust_castkms_capture_output)]
mod cases {
    use super::*;

    #[test]
    fn writes_complete_pixels_and_zeroes_only_described_padding() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let mut stream = Stream::new(&grantor.capture(), 1)?;
            let result = stream.capture()?;
            let image = destination(fixture, Layout::new(640, 480)?)?;
            result.copy_to_destination(&image, None)?;
            let pixels = pixels(image.buffer())?;
            check(pixels[..128].iter().all(|byte| *byte == 0x73))?;
            for y in 0..480 {
                let row = &pixels[128 + y * 2624..128 + (y + 1) * 2624];
                check(row[..2560].chunks_exact(4).enumerate().all(|(x, pixel)| {
                    pixel
                        == if y == 0 && x < 8 {
                            [0x12, 0x12, 0x12, 0xff]
                        } else {
                            [0, 0, 0, 0xff]
                        }
                }))?;
                check(row[2560..].iter().all(|byte| *byte == 0))?;
            }
            check(pixels[128 + 480 * 2624..].iter().all(|byte| *byte == 0x73))
        })
    }

    #[test]
    fn destination_backpressure_does_not_retain_a_scanout_read() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let mut stream = Stream::new(&grantor.capture(), 1)?;
            let result = stream.capture()?;
            let image = destination(fixture, Layout::new(640, 480)?)?;
            let mut reuse = ManualFence::new()?;
            check(result.copy_to_destination(&image, Some(&reuse.fence())) == Err(EAGAIN))?;
            check(pixels(image.buffer())?.iter().all(|byte| *byte == 0x73))?;
            let source: ARef<Source> = fixture
                .drm
                .device()
                .output
                .inspect_accepted(|accepted| accepted.map(|(source, _)| source.into()))
                .ok_or(EINVAL)?;
            {
                let admission = source.hold_admission()?;
                check(admission.prepared()?.is_some())?;
            }
            {
                let map = fb.vmap::<gem::Object>()?;
                io_project!(map.view(), [try: 0..32]).copy_from_slice(&[0xdd; 32]);
            }
            for _ in 0..8 {
                fixture.select(&fb, false, 0)?;
            }
            reuse.complete(Ok(()))?;
            result.copy_to_destination(&image, Some(&reuse.fence()))?;
            check(pixels(image.buffer())?[128..132] == [0x12, 0x12, 0x12, 0xff])
        })
    }

    #[test]
    fn failed_reuse_preserves_destination_contents() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let mut stream = Stream::new(&grantor.capture(), 1)?;
            let result = stream.capture()?;
            let image = destination(fixture, Layout::new(640, 480)?)?;
            let mut reuse = ManualFence::new()?;
            reuse.complete(Err(EIO))?;
            check(result.copy_to_destination(&image, Some(&reuse.fence())) == Err(EIO))?;
            check(pixels(image.buffer())?.iter().all(|byte| *byte == 0x73))
        })
    }

    #[test]
    fn pending_reuse_is_distinct_from_a_terminal_retry_error() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let mut stream = Stream::new(&grantor.capture(), 1)?;
            let result = stream.capture()?;
            let image = destination(fixture, Layout::new(640, 480)?)?;
            let mut reuse = ManualFence::new()?;
            check(result.try_copy_to_destination(&image, Some(&reuse.fence())) == Ok(false))?;
            reuse.complete(Err(EAGAIN))?;
            check(result.try_copy_to_destination(&image, Some(&reuse.fence())) == Err(EAGAIN))?;
            check(pixels(image.buffer())?.iter().all(|byte| *byte == 0x73))?;
            check(result.try_copy_to_destination(&image, None) == Ok(true))?;
            check(pixels(image.buffer())?[128..132] == [0x12, 0x12, 0x12, 0xff])
        })
    }

    #[test]
    fn wrong_layout_does_not_start_destination_access() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let mut stream = Stream::new(&grantor.capture(), 1)?;
            let result = stream.capture()?;
            let image = destination(fixture, Layout::new(639, 480)?)?;
            check(result.copy_to_destination(&image, None) == Err(EINVAL))?;
            check(pixels(image.buffer())?.iter().all(|byte| *byte == 0x73))
        })
    }

    #[test]
    fn revocation_preserves_completed_results_but_stream_close_discards_them() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let mut stream = Stream::new(&grantor.capture(), 1)?;
            let result = stream.capture()?;
            let image = destination(fixture, Layout::new(640, 480)?)?;
            drop(grantor);
            result.copy_to_destination(&image, None)?;
            check(pixels(image.buffer())?[128..132] == [0x12, 0x12, 0x12, 0xff])?;
            drop(stream);
            let fresh = destination(fixture, Layout::new(640, 480)?)?;
            check(result.copy_to_destination(&fresh, None) == Err(ENOENT))?;
            check(pixels(fresh.buffer())?.iter().all(|byte| *byte == 0x73))
        })
    }
}
