// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;

// Install a native association on a private internal client. The fixture does not make the
// file current master or publish it to other tasks. Native file close releases the reference.
fn associate(client: &HandleClient) -> Result {
    let file = client.file();
    assert!(file.associated_master().is_none());
    // SAFETY: The client retains its initialized device. The constructor returns one reference.
    let master =
        NonNull::new(unsafe { bindings::drm_master_create(file.device_raw()) }).ok_or(ENOMEM)?;
    // SAFETY: The private client's master is NULL and no other task accesses the file. Transfer
    // the newly allocated reference to the native file, whose close path will release it.
    unsafe { (*file.as_raw()).master = master.as_ptr() };
    Ok(())
}

#[kunit_tests(rust_drm_masters)]
mod tests {
    use super::*;

    #[test]
    fn unassociated_file_retains_no_device_reference() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-master-empty", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let client = HandleClient::new(&dev)?;
        let before = device_references(&dev);
        assert!(client.file().associated_master().is_none());
        assert_eq!(device_references(&dev), before);
        Ok(())
    }

    #[test]
    fn retained_identity_survives_file_close() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-master-lifetime", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let client = HandleClient::new(&dev)?;
        associate(&client)?;
        let before = device_references(&dev);
        let master = client.file().associated_master().ok_or(EINVAL)?;
        let second = client.file().associated_master().ok_or(EINVAL)?;
        assert!(master == second);
        let copy = master.clone();
        assert!(copy == master);
        assert_eq!(device_references(&dev), before + 3);
        drop(second);
        assert_eq!(device_references(&dev), before + 2);
        drop(client);
        drop(dev);
        assert!(counts.objects.load(Ordering::Relaxed) > 0);
        drop(master);
        assert!(counts.objects.load(Ordering::Relaxed) > 0);
        drop(copy);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn independent_associations_have_distinct_identity() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-master-distinct", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let first = HandleClient::new(&dev)?;
        let second = HandleClient::new(&dev)?;
        associate(&first)?;
        associate(&second)?;
        let a = first.file().associated_master().ok_or(EINVAL)?;
        let b = second.file().associated_master().ok_or(EINVAL)?;
        assert!(a != b);
        // SAFETY: Both private clients are non-master files with initialized lookup locks.
        assert!(!unsafe { bindings::drm_is_current_master(first.file().as_raw()) });
        assert!(!unsafe { bindings::drm_is_current_master(second.file().as_raw()) });
        Ok(())
    }
}
