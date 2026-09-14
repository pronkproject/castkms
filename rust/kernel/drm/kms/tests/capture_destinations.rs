// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Generic destination metadata through native PRIME exports, without pixel access.

mod client;

use super::*;
use crate::{
    dma_buf::DmaBuf,
    drm::{
        capture::{
            Destination,
            DestinationPlane, //
        },
        kms::testing::TestDevice, //
    }, //
};

fn with_buffer(read_only: bool, test: impl FnOnce(&DmaBuf) -> Result) -> Result {
    let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
    let parent = faux::Registration::new(c"rust-capture-destination", None)?;
    let result = (|| {
        let fixture = TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let buffer = if read_only {
            fixture.export_dumb_read_only(64, 64, 32)?
        } else {
            fixture.export_dumb(64, 64, 32)?
        };
        test(&buffer)
    })();
    // SAFETY: Every exported reference is gone. Drain final native file release outside
    // all locks while the exporter's parent remains bound, including on a failed check.
    unsafe { bindings::flush_delayed_fput() };
    result
}

#[kunit_tests(rust_drm_capture_destinations)]
mod cases {
    use super::*;

    #[test]
    fn temporary_plane_lists_preserve_exact_borrowed_storage() -> Result {
        with_buffer(false, |buffer| {
            let destination = Destination::new(
                [16, 16],
                fourcc::XRGB8888,
                fourcc::FORMAT_MOD_LINEAR,
                &[
                    DestinationPlane::new(buffer, 64, 0),
                    DestinationPlane::new(buffer, 128, 1024),
                ],
            )?;
            let plane = destination.plane(1).ok_or(EINVAL)?;
            if destination.dimensions() != [16, 16]
                || destination.format() != fourcc::XRGB8888
                || destination.modifier() != fourcc::FORMAT_MOD_LINEAR
                || destination.num_planes() != 2
                || destination.plane(2).is_some()
                || !core::ptr::eq(plane.buffer(), buffer)
                || plane.stride() != 128
                || plane.offset() != 1024
            {
                return Err(EINVAL);
            }
            Ok(())
        })
    }

    #[test]
    fn malformed_metadata_is_rejected_before_exposing_a_description() -> Result {
        with_buffer(false, |buffer| {
            let plane = DestinationPlane::new(buffer, 64, 0);
            for planes in [&[][..], &[plane; 5][..]] {
                if !matches!(
                    Destination::new([16, 16], fourcc::XRGB8888, 0, planes),
                    Err(EINVAL)
                ) {
                    return Err(EINVAL);
                }
            }
            for plane in [
                DestinationPlane::new(buffer, 0, 0),
                DestinationPlane::new(buffer, 64, buffer.size() as u64),
            ] {
                if !matches!(
                    Destination::new([16, 16], fourcc::XRGB8888, 0, &[plane]),
                    Err(EINVAL)
                ) {
                    return Err(EINVAL);
                }
            }
            Ok(())
        })
    }

    #[test]
    fn read_only_exports_do_not_form_writable_destination_descriptions() -> Result {
        with_buffer(true, |buffer| {
            if matches!(
                Destination::new(
                    [16, 16],
                    fourcc::XRGB8888,
                    0,
                    &[DestinationPlane::new(buffer, 64, 0)]
                ),
                Err(EACCES)
            ) {
                Ok(())
            } else {
                Err(EINVAL)
            }
        })
    }

    #[test]
    fn native_conversion_does_not_read_inactive_plane_storage() -> Result {
        with_buffer(false, |buffer| {
            let mut raw = core::mem::MaybeUninit::<bindings::drm_capture_destination>::uninit();
            let pointer = raw.as_mut_ptr();
            // SAFETY: Initialize the scalar fields and the sole active plane in exclusive
            // storage. Inactive plane entries deliberately remain uninitialized.
            unsafe {
                (&raw mut (*pointer).width).write(16);
                (&raw mut (*pointer).height).write(16);
                (&raw mut (*pointer).format).write(fourcc::XRGB8888);
                (&raw mut (*pointer).modifier).write(0);
                (&raw mut (*pointer).num_planes).write(1);
                (&raw mut (*pointer).planes[0]).write(bindings::drm_capture_destination_plane {
                    buffer: buffer.as_raw(),
                    stride: 64,
                    offset: 128,
                });
            }
            // SAFETY: Required metadata is initialized and stable. The active buffer
            // remains borrowed throughout this callback, longer than the copied view.
            let destination = unsafe { Destination::from_raw(pointer) }?;
            let plane = destination.plane(0).ok_or(EINVAL)?;
            if destination.num_planes() != 1
                || plane.offset() != 128
                || !core::ptr::eq(plane.buffer(), buffer)
            {
                return Err(EINVAL);
            }
            Ok(())
        })
    }
}
