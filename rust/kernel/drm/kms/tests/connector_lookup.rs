// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::drm::kms::testing::RegisteredMasterFile;
use connector::AsRawConnector;

#[kunit_tests(rust_drm_connector_lookup)]
mod cases {
    use super::*;

    #[test]
    fn lookup_retains_both_connector_and_device_until_final_release() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-connector-lookup", None)?;
        let foreign_parent = faux::Registration::new(c"rust-connector-lookup-foreign", None)?;
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
            // SAFETY: Completed setup published these observations and dev retains them.
            let (raw, id, wrong_type) = unsafe {
                let raw = dev.connector.load(Ordering::Relaxed);
                (
                    raw,
                    (*raw).base.id,
                    (*dev.crtc.load(Ordering::Relaxed)).base.id,
                )
            };
            assert!(matches!(dev.lookup_connector(owner.file(), 0), Err(ENOENT)));
            assert!(matches!(
                dev.lookup_connector(owner.file(), wrong_type),
                Err(ENOENT)
            ));
            assert!(matches!(
                other.lookup_connector(owner.file(), id),
                Err(EINVAL)
            ));
            let before = device_references(&dev);
            let found = dev.lookup_connector(owner.file(), id)?;
            assert_eq!(device_references(&dev), before + 1);
            assert_eq!(found.as_raw(), raw);
            let cloned = found.clone();
            assert_eq!(device_references(&dev), before + 2);
            drop(found);
            assert_eq!(device_references(&dev), before + 1);
            cloned
        };
        drop(first);
        drop(second);
        assert!(counts.objects.load(Ordering::Relaxed) > 0);
        drop(retained);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
