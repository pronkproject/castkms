// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Real master transitions while a temporary DRM device remains registered.

use super::*;
use crate::drm::kms::testing::RegisteredMasterFile;

#[kunit_tests(rust_drm_registered_master_files)]
mod cases {
    use super::*;

    #[test]
    fn native_open_close_and_competition_preserve_current_ownership() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-registered-master", None)?;
        // SAFETY: Registration is destroyed before the owning faux parent on every path.
        let registration = unsafe {
            drm::Registration::new_static(
                parent.as_ref().as_ref(),
                allocate(parent.as_ref(), &counts, false)?,
                Ok::<(), Error>(()),
                0,
            )?
        };
        {
            let registered = registration.registration_guard().ok_or(ENODEV)?;
            let first = RegisteredMasterFile::new(&registered)?;
            let identity = first.file().master_snapshot().ok_or(EINVAL)?;
            assert!(identity.was_current());
            assert_eq!(counts.master_sets.load(Ordering::Relaxed), 1);
            assert!(matches!(RegisteredMasterFile::new(&registered), Err(EBUSY)));
            assert_eq!(counts.master_sets.load(Ordering::Relaxed), 1);
            assert_eq!(counts.master_drops.load(Ordering::Relaxed), 0);
            assert!(identity.master().lock_current().is_some());
            drop(first);
            assert!(identity.master().lock_current().is_none());
            assert_eq!(counts.master_drops.load(Ordering::Relaxed), 1);
            let second = RegisteredMasterFile::new(&registered)?;
            assert!(second.file().master_snapshot().ok_or(EINVAL)?.was_current());
            assert_eq!(counts.master_sets.load(Ordering::Relaxed), 2);
            drop(second);
            assert_eq!(counts.master_drops.load(Ordering::Relaxed), 2);
        }
        drop(registration);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
