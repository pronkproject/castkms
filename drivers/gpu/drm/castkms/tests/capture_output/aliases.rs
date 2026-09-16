// SPDX-License-Identifier: GPL-2.0-only

//! Known source aliases are rejected at admission without consuming request names.

use super::*;
use kernel::{
    drm::{
        capture::{
            ClientStream,
            Description, //
        },
        gem::IntoGEMObject, //
    },
    error::from_err_ptr, //
};

fn export(fb: &FramebufferRef<Driver>) -> Result<Image> {
    let object = fb.object_at(0)?;
    // SAFETY: The private fixture retains this initialized local GEM object. Native
    // PRIME export acquires independent GEM/device references and installs no descriptor.
    let raw = from_err_ptr(unsafe {
        kernel::bindings::drm_gem_prime_export(object.as_raw(), kernel::bindings::O_RDWR as i32)
    })?;
    let raw = core::ptr::NonNull::new(raw).ok_or(ENOMEM)?;
    // SAFETY: Native export transferred one owned reference to a transparent DmaBuf.
    let buffer = unsafe { ARef::<DmaBuf>::from_raw(raw.cast()) };
    Image::new(
        buffer,
        Layout::new(640, 480)?,
        fourcc::XRGB8888,
        fourcc::FORMAT_MOD_LINEAR,
        2560,
        0,
    )
}

#[kunit_tests(rust_castkms_capture_destination_alias)]
mod cases {
    use super::super::file::{
        register,
        with_client, //
    };
    use super::*;

    #[test]
    fn distinct_exports_of_the_selected_source_leave_the_request_retryable() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let creator = fixture.drm.master_file()?;
            let fb = select(fixture, &creator)?;
            let grantor = grant(fixture, &creator)?;
            let first = export(&fb)?;
            let second = export(&fb)?;
            check(!core::ptr::eq(first.buffer(), second.buffer()))?;
            check(core::ptr::eq(
                first.buffer().reservation(),
                second.buffer().reservation(),
            ))?;
            with_client(grantor.capture(), |file| {
                let mut first_registration = register(file, 1, &first)?;
                check(register(file, 2, &second).err() == Some(EEXIST))?;
                let mut stream = ClientStream::open(file, 1, Description::query(file)?.id(), 1)?;
                check(stream.queue_output(1, 1, None) == Err(EINVAL))?;
                first_registration.unregister()?;
                let _second = register(file, 2, &second)?;
                check(stream.queue_output(1, 2, None) == Err(EINVAL))?;
                let independent = destination(fixture, Layout::new(640, 480)?)?;
                let _independent = register(file, 3, &independent)?;
                let reuse = ManualFence::new()?;
                stream.queue_output(1, 3, Some(&reuse.fence()))?;
                stream.close()
            })
        })
    }

    #[test]
    fn source_selection_after_registration_is_checked_when_output_is_queued() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let creator = fixture.drm.master_file()?;
            let _old = select(fixture, &creator)?;
            let grantor = grant(fixture, &creator)?;
            let next = fixture.framebuffer(provenance::Provenance::from_snapshot(
                creator.file().master_snapshot(),
            ))?;
            let image = export(&next)?;
            with_client(grantor.capture(), |file| {
                let _registered = register(file, 1, &image)?;
                let mut stream = ClientStream::open(file, 1, Description::query(file)?.id(), 1)?;
                fixture.select(&next, false, 0)?;
                check(stream.queue_output(1, 1, None) == Err(EINVAL))?;
                stream.close()
            })
        })
    }
}
