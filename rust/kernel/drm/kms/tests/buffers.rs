// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::drm::kms::testing::TestDevice;

#[kunit_tests(rust_drm_kms_buffer_exports)]
mod cases {
    use super::*;

    #[test]
    fn exported_storage_outlives_private_client_and_fixture() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-buffer-export", None)?;
        let fixture = TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let buffer = fixture.export_dumb(64, 64, 32)?;
        assert_eq!(buffer.size(), 16384);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 1);
        drop(fixture);
        assert_eq!(buffer.size(), 16384);
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 1);
        drop(buffer);
        // SAFETY: Drain the private export's deferred file release in the KUnit kernel thread
        // with no object or reservation locks held, while its faux parent remains bound.
        unsafe { bindings::flush_delayed_fput() };
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn invalid_export_dimensions_leave_no_private_handle_objects() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-kms-buffer-reject", None)?;
        let fixture = TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        assert!(fixture.export_dumb(0, 64, 32).is_err());
        assert!(fixture.export_dumb(64, 0, 32).is_err());
        assert_eq!(counts.gem_objects.load(Ordering::Relaxed), 0);
        assert_eq!(counts.gem_creations.load(Ordering::Relaxed), 0);
        drop(fixture);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
