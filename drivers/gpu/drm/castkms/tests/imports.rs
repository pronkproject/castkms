// SPDX-License-Identifier: GPL-2.0-only

//! Registered CastKMS import using private foreign exports, without reading pixels.

use super::*;
use kernel::drm::{
    gem::BaseObject,
    kms::framebuffer::Framebuffer, //
};

#[kunit_tests(rust_castkms_import)]
mod cases {
    use super::*;

    #[test]
    fn registered_driver_retains_foreign_storage_for_framebuffer_metadata() -> Result {
        with_exporter(|source| {
            let buffer = source.drm.export_dumb(64, 64, 32)?;
            check(buffer.size() == 16384)?;
            let target = CastKms::new(c"castkms-foreign-import-test")?;
            let object = {
                let registered = target._display.registration_guard().ok_or(ENODEV)?;
                shmem::Object::<gem::Object>::import(&registered, &buffer)?
            };
            check(object.size() == buffer.size())?;
            let retained = object.imported_dma_buf().ok_or(EINVAL)?;
            check(core::ptr::eq(&*retained, &*buffer))?;
            let framebuffer = {
                let registered = target._display.registration_guard().ok_or(ENODEV)?;
                Framebuffer::from_objects(
                    &registered,
                    &FramebufferLayout {
                        width: 64,
                        height: 64,
                        format: drm::fourcc::XRGB8888,
                        modifier: Some(drm::fourcc::FORMAT_MOD_LINEAR),
                        interlaced: false,
                        planes: &[FramebufferPlane {
                            object: &object,
                            pitch: 256,
                            offset: 0,
                        }],
                    },
                )?
            };
            check(framebuffer.width() == 64)?;
            check(framebuffer.height() == 64)?;
            check(framebuffer.modifier() == Some(drm::fourcc::FORMAT_MOD_LINEAR))?;
            check(framebuffer.offset(0)? == 0)?;
            // Imported storage need not expose a persistent CPU mapping.
            check(framebuffer.owned_vmap::<gem::Object>().is_err())?;
            drop(buffer);
            drop(target);
            check(object.size() == 16384)?;
            check(retained.size() == 16384)?;
            drop(framebuffer);
            drop(object);
            check(retained.size() == 16384)?;
            drop(retained);
            Ok(())
        })
    }

    #[test]
    fn fixture_export_rejects_invalid_dumb_dimensions() -> Result {
        with_exporter(|source| {
            check(source.drm.export_dumb(0, 64, 32).is_err())?;
            check(source.drm.export_dumb(64, 0, 32).is_err())?;
            let buffer = source.drm.export_dumb(64, 64, 32)?;
            check(buffer.size() == 16384)?;
            drop(buffer);
            Ok(())
        })
    }
}
