// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::drm::kms::testing::RegisteredMasterFile;
use crtc::AsRawCrtc;

#[kunit_tests(rust_drm_crtc_lookup)]
mod cases {
    use super::*;

    #[test]
    fn lookup_checks_file_and_type_then_retains_the_device() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-crtc-lookup", None)?;
        let foreign_parent = faux::Registration::new(c"rust-crtc-lookup-foreign", None)?;
        // SAFETY: Both registrations and every returned object are released before parent.
        let first = unsafe {
            drm::Registration::new_static(
                parent.as_ref().as_ref(),
                allocate(parent.as_ref(), &counts, false)?,
                Ok::<(), Error>(()),
                0,
            )?
        };
        // SAFETY: The second registration has the same parent lifetime discipline.
        let second = unsafe {
            drm::Registration::new_static(
                foreign_parent.as_ref().as_ref(),
                allocate(foreign_parent.as_ref(), &counts, false)?,
                Ok::<(), Error>(()),
                0,
            )?
        };
        let retained = {
            let dev = first.registration_guard().ok_or(ENODEV)?;
            let other = second.registration_guard().ok_or(ENODEV)?;
            let owner = RegisteredMasterFile::new(&dev)?;
            // SAFETY: Registered setup published these observations; dev retains the objects.
            let (id, wrong_type) = unsafe {
                (
                    (*dev.crtc.load(Ordering::Relaxed)).base.id,
                    (*dev.connector.load(Ordering::Relaxed)).base.id,
                )
            };
            assert!(matches!(dev.lookup_crtc(owner.file(), 0), Err(ENOENT)));
            assert!(matches!(
                dev.lookup_crtc(owner.file(), wrong_type),
                Err(ENOENT)
            ));
            assert!(matches!(other.lookup_crtc(owner.file(), id), Err(EINVAL)));
            let before = device_references(&dev);
            let found = dev.lookup_crtc(owner.file(), id)?;
            assert_eq!(device_references(&dev), before + 1);
            assert_eq!(found.crtc().as_raw(), dev.crtc.load(Ordering::Relaxed));
            found
        };
        drop(first);
        drop(second);
        assert!(counts.objects.load(Ordering::Relaxed) > 0);
        drop(retained);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
