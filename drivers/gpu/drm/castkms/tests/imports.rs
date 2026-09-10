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
        let source = Fixture::new()?;
        let buffer = source.drm.export_dumb(64, 64, 32)?;
        assert_eq!(buffer.size(), 16384);
        let target = CastKms::new(c"castkms-foreign-import-test")?;
        let object = {
            let registered = target._display.registration_guard().ok_or(ENODEV)?;
            shmem::Object::<gem::Object>::import(&registered, &buffer)?
        };
        assert_eq!(object.size(), buffer.size());
        let retained = object.imported_dma_buf().ok_or(EINVAL)?;
        assert!(core::ptr::eq(&*retained, &*buffer));
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
        assert_eq!(framebuffer.width(), 64);
        assert_eq!(framebuffer.height(), 64);
        assert_eq!(framebuffer.modifier(), Some(drm::fourcc::FORMAT_MOD_LINEAR));
        assert_eq!(framebuffer.offset(0)?, 0);
        // Imported storage need not expose a persistent CPU mapping.
        assert!(framebuffer.owned_vmap::<gem::Object>().is_err());
        drop(buffer);
        drop(target);
        assert_eq!(object.size(), 16384);
        assert_eq!(retained.size(), 16384);
        drop(framebuffer);
        drop(object);
        assert_eq!(retained.size(), 16384);
        drop(retained);
        // SAFETY: KUnit runs in a kernel thread with no reservation or device locks held.
        // Drain deferred DMA-BUF release before unbinding the foreign exporter's parent.
        unsafe { kernel::bindings::flush_delayed_fput() };
        // Keep the foreign exporter's faux parent bound until native detach and release end.
        drop(source);
        Ok(())
    }

    #[test]
    fn fixture_export_rejects_invalid_dumb_dimensions() -> Result {
        let source = Fixture::new()?;
        assert!(source.drm.export_dumb(0, 64, 32).is_err());
        assert!(source.drm.export_dumb(64, 0, 32).is_err());
        let buffer = source.drm.export_dumb(64, 64, 32)?;
        assert_eq!(buffer.size(), 16384);
        drop(buffer);
        // SAFETY: Drain the private export's deferred release without holding any locks.
        unsafe { kernel::bindings::flush_delayed_fput() };
        drop(source);
        Ok(())
    }
}
