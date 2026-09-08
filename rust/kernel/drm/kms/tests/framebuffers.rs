// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;

pub(super) struct Metadata {
    counts: Arc<Counts>,
    from_file: bool,
}

impl Metadata {
    pub(super) fn new(counts: &Arc<Counts>, from_file: bool) -> Self {
        Self {
            counts: counts.clone(),
            from_file,
        }
    }
}

impl Drop for Metadata {
    fn drop(&mut self) {
        self.counts
            .framebuffer_data_drops
            .fetch_add(1, Ordering::Relaxed);
    }
}

#[kunit_tests(rust_drm_framebuffer_metadata)]
mod tests {
    use super::*;

    fn assert_device_vtable(fb: &framebuffer::Framebuffer<TestDriver>) {
        // SAFETY: The live framebuffer belongs to the initialized Rust test device. Its
        // mode-config pointer names the selected ModeConfigOps allocation for that device.
        let expected = unsafe {
            let ops = crate::container_of!(
                (*fb.drm_dev().as_raw()).mode_config.funcs,
                ModeConfigOps,
                kms_vtable
            );
            &raw const (*ops).framebuffer_vtable
        };
        // SAFETY: Successful framebuffer initialization installs immutable callbacks.
        assert_eq!(unsafe { (*fb.as_raw()).funcs }, expected);
    }

    #[test]
    fn metadata_lives_until_final_framebuffer_reference() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-fb-metadata-lifetime", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let fb = framebuffer(&dev)?;
        assert_device_vtable(&fb);
        assert!(!fb.data().ok_or(EINVAL)?.from_file);
        let retained = fb.clone();
        drop(fb);
        drop(dev);
        assert_eq!(counts.framebuffer_data_drops.load(Ordering::Relaxed), 0);
        drop(retained);
        assert_eq!(counts.framebuffer_data_drops.load(Ordering::Relaxed), 1);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn invalid_storage_releases_metadata() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-fb-metadata-reject", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let object = gem::shmem::Object::<TestObject>::new(&dev, 4096, Default::default(), ())?;
        assert!(framebuffer_with_object(&dev, object.clone(), 640, 480).is_err());
        assert_eq!(counts.framebuffer_data_drops.load(Ordering::Relaxed), 1);
        drop(object);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn metadata_failure_does_not_retain_storage() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-fb-metadata-failure", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        counts.fail_framebuffer_data.store(1, Ordering::Relaxed);
        assert!(framebuffer(&dev).is_err());
        assert_eq!(counts.framebuffer_data_drops.load(Ordering::Relaxed), 0);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn kernel_caller_supplies_metadata_without_file() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-fb-metadata-kernel", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let object = gem::shmem::Object::<TestObject>::new(&dev, 4096, Default::default(), ())?;
        let planes = [framebuffer::FramebufferPlane {
            object: &*object,
            pitch: 64,
            offset: 0,
        }];
        let layout = framebuffer::FramebufferLayout {
            width: 16,
            height: 16,
            format: fourcc::XRGB8888,
            modifier: None,
            interlaced: false,
            planes: &planes,
        };
        // SAFETY: The private unregistered fixture exclusively protects completed KMS setup.
        let fb = unsafe {
            framebuffer::Framebuffer::from_objects_with_data_unchecked(
                &dev,
                &layout,
                Metadata::new(&counts, true),
            )
        }?;
        assert!(fb.data().ok_or(EINVAL)?.from_file);
        drop(fb);
        assert_eq!(counts.framebuffer_data_drops.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[cfg(CONFIG_DRM_CLIENT)]
    #[test]
    fn file_constructor_attaches_metadata() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-fb-metadata-file", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let client = HandleClient::new(&dev)?;
        let mut dumb = bindings::drm_mode_create_dumb {
            width: 64,
            height: 64,
            bpp: 32,
            ..Default::default()
        };
        // SAFETY: The private file and device match, and the argument is writable.
        crate::error::to_result(unsafe {
            (*(*dev.as_raw()).driver).dumb_create.unwrap()(
                client.file().as_raw(),
                dev.as_raw(),
                &mut dumb,
            )
        })?;
        let command = bindings::drm_mode_fb_cmd2 {
            width: 64,
            height: 64,
            pixel_format: fourcc::XRGB8888,
            handles: [dumb.handle, 0, 0, 0],
            pitches: [dumb.pitch, 0, 0, 0],
            ..Default::default()
        };
        // SAFETY: Valid packed layout and live handle, on a private initialized KMS device.
        let raw = crate::error::from_err_ptr(unsafe {
            framebuffer::create_callback::<TestDriver>(
                dev.as_raw(),
                client.file().as_raw(),
                bindings::drm_format_info(fourcc::XRGB8888),
                &command,
            )
        })?;
        // SAFETY: Successful constructor returns one framebuffer reference on TestDriver.
        let fb = unsafe { framebuffer::Framebuffer::<TestDriver>::from_raw(raw) }.to_owned_ref();
        // SAFETY: Transfer the initial native reference to the device-retaining Rust owner.
        unsafe { bindings::drm_framebuffer_put(raw) };
        assert_device_vtable(&fb);
        assert!(fb.data().ok_or(EINVAL)?.from_file);
        drop(client);
        assert_eq!(counts.framebuffer_data_drops.load(Ordering::Relaxed), 0);
        drop(fb);
        assert_eq!(counts.framebuffer_data_drops.load(Ordering::Relaxed), 1);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn native_generic_framebuffer_has_no_typed_metadata() -> Result {
        use gem::IntoGEMObject;

        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-fb-metadata-native", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let object = gem::shmem::Object::<TestObject>::new(&dev, 4096, Default::default(), ())?;
        let command = bindings::drm_mode_fb_cmd2 {
            width: 16,
            height: 16,
            pixel_format: fourcc::XRGB8888,
            pitches: [64, 0, 0, 0],
            ..Default::default()
        };
        let objects = [object.as_raw()];
        // SAFETY: The private test device has stable KMS setup; objects are borrowed for
        // the call. This constructor allocates only the generic native framebuffer.
        let raw = crate::error::from_err_ptr(unsafe {
            bindings::drm_gem_fb_create_from_objects(dev.as_raw(), &command, objects.as_ptr(), 1)
        })?;
        // SAFETY: The constructor returned a framebuffer on TestDriver with one reference.
        let fb = unsafe { framebuffer::Framebuffer::<TestDriver>::from_raw(raw) }.to_owned_ref();
        // SAFETY: Transfer the native reference to the paired framebuffer/device owner.
        unsafe { bindings::drm_framebuffer_put(raw) };
        assert!(fb.data().is_none());
        drop(fb);
        drop(object);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.framebuffer_data_drops.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
