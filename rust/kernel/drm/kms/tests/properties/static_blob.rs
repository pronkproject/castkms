// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Fixed blob descriptions and cleanup in a private connector construction interval.

use super::*;

#[kunit_tests(rust_drm_connector_static_blob)]
mod cases {
    use super::*;

    #[test]
    fn description_copies_bytes_before_publication() -> Result {
        let ((flags, bytes, state_absent), counts) = with_fresh_connector(|connector, dev, _| {
            let mut source = [1u8, 2, 3, 4];
            connector.attach_static_blob_property(c"test-description", &source)?;
            source.fill(0);
            // SAFETY: The private connector's initialized property array is unchanged while read.
            let (property, id, state_absent) = unsafe {
                let raw = connector.as_raw();
                let properties = &*(*raw).base.properties;
                let index = properties.count as usize - 1;
                (
                    properties.properties[index],
                    properties.values[index] as u32,
                    (*raw).state.is_null(),
                )
            };
            // SAFETY: The private device and property remain live until the callback returns.
            let flags = unsafe { (*property).flags };
            // SAFETY: Mode configuration is initialized and id names the newly created blob.
            let blob = unsafe { bindings::drm_property_lookup_blob(dev.as_raw(), id) };
            if blob.is_null() {
                return Err(ENOENT);
            }
            // SAFETY: Lookup owns a reference to a blob initialized from four readable bytes.
            let bytes = unsafe { *(*blob).data.cast::<[u8; 4]>() };
            // SAFETY: Release only the lookup reference; mode configuration retains the initial one.
            unsafe { bindings::drm_property_blob_put(blob) };
            Ok((flags, bytes, state_absent))
        })?;
        assert_eq!(
            flags,
            bindings::DRM_MODE_PROP_BLOB | bindings::DRM_MODE_PROP_IMMUTABLE
        );
        assert_eq!(bytes, [1, 2, 3, 4]);
        assert!(state_absent);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn invalid_description_does_not_attach_a_property() -> Result {
        let ((errors, before, after), counts) = with_fresh_connector(|connector, _, _| {
            // SAFETY: The private initialized connector is exclusively accessed.
            let before = unsafe { (*(*connector.as_raw()).base.properties).count };
            let errors = [
                connector.attach_static_blob_property(c"", &[1]).err(),
                connector
                    .attach_static_blob_property(c"01234567890123456789012345678901", &[1])
                    .err(),
                connector.attach_static_blob_property(c"empty", &[]).err(),
            ];
            // SAFETY: Same private connector lifetime after all attachment attempts returned.
            let after = unsafe { (*(*connector.as_raw()).base.properties).count };
            Ok((errors, before, after))
        })?;
        assert_eq!(errors, [Some(EINVAL); 3]);
        assert_eq!(before, after);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn full_property_array_rejects_another_description() -> Result {
        let (error, counts) = with_fresh_connector(|connector, _, _| {
            // SAFETY: The private connector is not concurrently modified.
            let count = unsafe { (*(*connector.as_raw()).base.properties).count };
            for _ in count..bindings::DRM_OBJECT_MAX_PROPERTY as i32 {
                connector.attach_static_blob_property(c"test-slot", &[1])?;
            }
            Ok(connector
                .attach_static_blob_property(c"overflow", &[2])
                .err())
        })?;
        assert_eq!(error, Some(ENOSPC));
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
