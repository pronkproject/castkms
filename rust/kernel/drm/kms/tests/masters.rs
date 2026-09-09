// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;

mod transitions;

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
    fn absent_snapshot_retains_no_device_reference() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-master-snapshot-empty", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let client = HandleClient::new(&dev)?;
        let before = device_references(&dev);
        assert!(client.file().master_snapshot().is_none());
        assert_eq!(device_references(&dev), before);
        Ok(())
    }

    #[test]
    fn associated_snapshot_is_not_current() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-master-snapshot-client", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let client = HandleClient::new(&dev)?;
        associate(&client)?;
        let identity = client.file().associated_master().ok_or(EINVAL)?;
        let snapshot = client.file().master_snapshot().ok_or(EINVAL)?;
        assert!(!snapshot.was_current());
        assert!(snapshot.master() == &identity);
        drop(client);
        drop(dev);
        drop(identity);
        assert!(counts.objects.load(Ordering::Relaxed) > 0);
        drop(snapshot);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }

    #[test]
    fn current_snapshot_remains_historical_after_drop() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-master-snapshot-drop", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let client = HandleClient::new(&dev)?;
        associate(&client)?;
        let file = client.file();
        // SAFETY: The unregistered device remains private to this task.
        assert!(unsafe { (*dev.as_raw()).master.is_null() });
        // SAFETY: Give the private device a reference to the file's initialized master.
        unsafe {
            (*file.as_raw()).is_master = true;
            (*dev.as_raw()).master = bindings::drm_master_get((*file.as_raw()).master);
        }
        let current = file.master_snapshot().ok_or(EINVAL)?;
        assert!(current.was_current());
        // SAFETY: The private fixture still excludes concurrent access. Release the device's
        // active-master reference without changing the file's retained association.
        unsafe { bindings::drm_master_put(&raw mut (*dev.as_raw()).master) };
        let inactive = file.master_snapshot().ok_or(EINVAL)?;
        assert!(!inactive.was_current());
        assert!(current.master() == inactive.master());
        assert!(current.was_current());
        Ok(())
    }

    #[test]
    fn sharing_active_identity_does_not_make_a_client_current() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-master-snapshot-peer", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let owner = HandleClient::new(&dev)?;
        let peer = HandleClient::new(&dev)?;
        associate(&owner)?;
        // SAFETY: The device and peer client are private and remain live throughout setup.
        let (device_master, peer_master) =
            unsafe { ((*dev.as_raw()).master, (*peer.file().as_raw()).master) };
        assert!(device_master.is_null());
        assert!(peer_master.is_null());
        // SAFETY: Each native owner receives its own reference; native close releases it.
        unsafe {
            let master = (*owner.file().as_raw()).master;
            (*owner.file().as_raw()).is_master = true;
            (*dev.as_raw()).master = bindings::drm_master_get(master);
            (*peer.file().as_raw()).master = bindings::drm_master_get(master);
        }
        let owner_snapshot = owner.file().master_snapshot().ok_or(EINVAL)?;
        let peer_snapshot = peer.file().master_snapshot().ok_or(EINVAL)?;
        assert!(owner_snapshot.was_current());
        assert!(!peer_snapshot.was_current());
        assert!(owner_snapshot.master() == peer_snapshot.master());
        Ok(())
    }

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
