// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Property byte ownership is independent of connector publication and registration.

use super::*;
use blob::Blob;

#[kunit_tests(rust_drm_property_blobs)]
mod cases {
    use super::*;

    #[test]
    fn copied_bytes_and_device_lifetime_survive_unplug() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-property-blob", None)?;
        // SAFETY: Registration and retained blobs are released before their faux parent.
        let registration = unsafe {
            drm::Registration::new_static(
                parent.as_ref().as_ref(),
                allocate(parent.as_ref(), &counts, false)?,
                Ok::<(), Error>(()),
                0,
            )?
        };
        let (blob, invalid, copied, nonzero, same_id) = {
            let registered = registration.registration_guard().ok_or(ENODEV)?;
            let invalid = Blob::new(&registered, &[]).err();
            let mut bytes = [1, 3, 5, 7];
            let blob = Blob::new(&registered, &bytes)?;
            bytes.fill(0);
            let copied = blob.as_bytes() == [1, 3, 5, 7];
            let nonzero = blob.id() != 0;
            let clone = blob.clone();
            let same_id = clone.id() == blob.id();
            drop(blob);
            (clone, invalid, copied, nonzero, same_id)
        };
        drop(registration);
        let retained_objects = counts.objects.load(Ordering::Relaxed);
        let retained_bytes = blob.as_bytes() == [1, 3, 5, 7];
        drop(blob);
        assert_eq!(invalid, Some(EINVAL));
        assert!(copied && nonzero && same_id && retained_bytes);
        assert!(retained_objects > 0);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
