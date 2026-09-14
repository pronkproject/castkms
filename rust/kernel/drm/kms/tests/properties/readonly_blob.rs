// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Preallocated blob replacement transfers retired ownership without releasing it under locks.

use super::*;
use crate::drm::kms::{
    blob::Blob,
    lock::with_locked_state, //
};

// The fixture excludes object creation and teardown; callers also exclude property changes.
fn published(
    connector: &connector::UnregisteredConnector<TestConnector>,
    dev: &Device<TestDriver>,
    property_id: u32,
) -> Result<(u32, [u8; 4])> {
    // SAFETY: The initialized property array remains live and stable in this private fixture.
    let id = unsafe {
        let properties = &*(*connector.as_raw()).base.properties;
        (0..properties.count as usize)
            .find(|&index| (*properties.properties[index]).base.id == property_id)
            .map(|index| properties.values[index] as u32)
            .ok_or(ENOENT)?
    };
    // SAFETY: The private initialized device owns the published object registry.
    let blob = unsafe { bindings::drm_property_lookup_blob(dev.as_raw(), id) };
    if blob.is_null() {
        return Err(ENOENT);
    }
    // SAFETY: Lookup retains immutable data; check the length before reading four bytes.
    let bytes = unsafe {
        if (*blob).length == 4 {
            Ok(*(*blob).data.cast::<[u8; 4]>())
        } else {
            Err(EINVAL)
        }
    };
    // SAFETY: Release the lookup reference; this helper is not called under object-ID locks.
    unsafe { bindings::drm_property_blob_put(blob) };
    Ok((id, bytes?))
}

#[kunit_tests(rust_drm_connector_readonly_blob)]
mod cases {
    use super::*;

    #[test]
    fn replacement_returns_retired_bytes_and_survives_handle_drop() -> Result {
        let ((before, after, retired, kept, after_drop), counts) =
            with_fresh_connector(|connector, dev, _| {
                let mut property =
                    connector.attach_readonly_blob_property(c"driver-description", &[1; 4])?;
                let property_id = property.id();
                let before = published(connector, dev, property_id)?;
                // SAFETY: The fixture owns initialized mode configuration and holds no locks.
                let mut replacement = unsafe { Blob::new_unchecked(dev, &[2; 4]) }?;
                let kept = replacement.clone();
                // SAFETY: Static setup is complete, the private fixture excludes teardown,
                // and the callback neither waits nor releases native references.
                unsafe {
                    with_locked_state(dev, |state| property.replace_blob(state, &mut replacement))
                }??;
                let after = published(connector, dev, property_id)?;
                let retired = (replacement.id(), replacement.as_bytes() == [1; 4]);
                let kept_data = (kept.id(), kept.as_bytes() == [2; 4]);
                drop(kept);
                drop(replacement);
                drop(property);
                let after_drop = published(connector, dev, property_id)?;
                Ok((before, after, retired, kept_data, after_drop))
            })?;
        assert_eq!(before.1, [1; 4]);
        assert_eq!(after.1, [2; 4]);
        assert_ne!(before.0, after.0);
        assert_eq!(retired, (before.0, true));
        assert_eq!(kept, (after.0, true));
        assert_eq!(after_drop, after);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn foreign_lock_and_replacement_leave_both_owners_unchanged() -> Result {
        let ((first_error, second_error, before, after, own, foreign), counts) =
            with_fresh_connector(|connector, dev, _| {
                let foreign_counts = Arc::new(Counts::default(), GFP_KERNEL)?;
                let parent = faux::Registration::new(c"rust-blob-other", None)?;
                let other = create(parent.as_ref(), &foreign_counts, false)?;
                let mut property =
                    connector.attach_readonly_blob_property(c"driver-description", &[1; 4])?;
                let before = published(connector, dev, property.id())?;
                // SAFETY: Both devices have initialized private mode configurations and no locks.
                let mut own = unsafe { Blob::new_unchecked(dev, &[2; 4]) }?;
                let mut foreign = unsafe { Blob::new_unchecked(&other, &[3; 4]) }?;
                // SAFETY: Each fixture excludes teardown and the callbacks only validate ownership.
                let first_error = unsafe {
                    with_locked_state(&other, |state| property.replace_blob(state, &mut own))
                }?
                .err();
                let second_error = unsafe {
                    with_locked_state(dev, |state| property.replace_blob(state, &mut foreign))
                }?
                .err();
                let after = published(connector, dev, property.id())?;
                let own_valid = own.as_bytes() == [2; 4];
                let foreign_valid = foreign.as_bytes() == [3; 4];
                Ok((first_error, second_error, before, after, own_valid, foreign_valid))
            })?;
        assert_eq!(first_error, Some(EINVAL));
        assert_eq!(second_error, Some(EINVAL));
        assert_eq!(before, after);
        assert!(own && foreign);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn invalid_initial_data_does_not_attach_a_property() -> Result {
        let ((error, before, after), counts) = with_fresh_connector(|connector, _, _| {
            // SAFETY: The fixture exclusively accesses the initialized property array.
            let before = unsafe { (*(*connector.as_raw()).base.properties).count };
            let error = connector
                .attach_readonly_blob_property(c"driver-description", &[])
                .err();
            let after = unsafe { (*(*connector.as_raw()).base.properties).count };
            Ok((error, before, after))
        })?;
        assert_eq!(error, Some(EINVAL));
        assert_eq!(before, after);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
