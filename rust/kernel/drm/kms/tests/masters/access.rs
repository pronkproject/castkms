// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crtc::AsRawCrtc;

// The fixture's internal client is private and owns its associated master until file close.
fn make_current(client: &HandleClient) -> Result {
    let file = client.file();
    let dev = file.device_raw();
    // SAFETY: The unregistered device and client are private to the calling test.
    if unsafe { !(*dev).master.is_null() || !(*file.as_raw()).master.is_null() } {
        return Err(EBUSY);
    }
    // SAFETY: The client retains the initialized device. The constructor returns one reference.
    let master = NonNull::new(unsafe { bindings::drm_master_create(dev) }).ok_or(ENOMEM)?;
    // SAFETY: No task observes the private file during setup. Transfer the new reference to
    // the file and give the device its own reference, under the native master mutex. Native
    // file close releases both references and invokes the normal master-drop callback.
    unsafe {
        bindings::mutex_lock(&raw mut (*dev).master_mutex);
        (*file.as_raw()).master = master.as_ptr();
        (*file.as_raw()).is_master = true;
        (*file.as_raw()).was_master = true;
        (*dev).master = bindings::drm_master_get(master.as_ptr());
        bindings::mutex_unlock(&raw mut (*dev).master_mutex);
    }
    Ok(())
}

// Attach a private client's identity to its retained lessor, with no leased objects.
fn associate_lessee(client: &HandleClient, owner: &HandleClient) -> Result {
    let file = client.file();
    let dev = file.device_raw();
    if dev != owner.file().device_raw() {
        return Err(EINVAL);
    }
    // SAFETY: Both clients are private to the fixture and remain live throughout setup.
    let lessor = unsafe { (*owner.file().as_raw()).master };
    // SAFETY: The private client has not published an association to another task.
    if lessor.is_null() || unsafe { !(*file.as_raw()).master.is_null() } {
        return Err(EINVAL);
    }
    // SAFETY: The client retains the initialized device; success returns one master reference.
    let master = NonNull::new(unsafe { bindings::drm_master_create(dev) }).ok_or(ENOMEM)?;
    // SAFETY: Initialize the new identity's immutable lessor before publishing it to the file.
    // The native object-ID lock protects the lessor's list. Each link is live and initialized;
    // native master destruction removes the list entry and releases the lessor reference.
    unsafe {
        bindings::mutex_lock(&raw mut (*dev).mode_config.idr_mutex);
        (*master.as_ptr()).lessor = bindings::drm_master_get(lessor);
        bindings::list_add_tail(
            &raw mut (*master.as_ptr()).lessee_list,
            &raw mut (*lessor).lessees,
        );
        bindings::mutex_unlock(&raw mut (*dev).mode_config.idr_mutex);
        (*file.as_raw()).master = master.as_ptr();
    }
    Ok(())
}

#[track_caller]
fn check(condition: bool) -> Result {
    if condition {
        Ok(())
    } else {
        let location = core::panic::Location::caller();
        pr_err!(
            "Master access check failed at {}:{}\n",
            location.file(),
            location.line()
        );
        Err(EINVAL)
    }
}

#[kunit_tests(rust_drm_master_access)]
mod tests {
    use super::*;

    #[test]
    fn identity_exclusion_allows_object_checks_inside_modeset_validation() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-master-modeset-order", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let client = HandleClient::new(&dev)?;
        make_current(&client)?;
        let master = client.file().associated_master().ok_or(EINVAL)?;
        {
            let identity = master.lock_current_identity().ok_or(EACCES)?;
            check(identity.with_objects(|_| Err::<(), _>(EIO)) == Err(EIO))?;
            // SAFETY: The fixture retains its only initialized CRTC, excluding teardown.
            let crtc =
                unsafe { crtc::Crtc::<TestCrtc>::from_raw(dev.crtc.load(Ordering::Relaxed)) };
            // SAFETY: The fixture owns completed KMS setup. The callback takes object-ID
            // exclusion only after the transaction acquires the CRTC modeset lock.
            unsafe {
                atomic::run_check(&dev, |transaction| {
                    drop(transaction.add_crtc_state(crtc)?);
                    check(identity.with_objects(|objects| {
                        objects.holds_object(crtc) && objects.is_master_file(client.file())
                    }))
                })
            }?;
        }
        check(master.lock_current().is_some())?;
        drop(client);
        check(master.lock_current_identity().is_none())?;
        Ok(())
    }

    #[test]
    fn a_master_file_is_distinct_from_clients_sharing_its_identity() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-master-file-role", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let other = create(parent.as_ref(), &counts, false)?;
        let owner = HandleClient::new(&dev)?;
        let peer = HandleClient::new(&dev)?;
        let unassociated = HandleClient::new(&dev)?;
        let foreign = HandleClient::new(&other)?;
        make_current(&owner)?;
        make_current(&foreign)?;
        // SAFETY: The peer is private with no association. Transfer one reference from the
        // owner's live identity; native client close releases it. Its master role stays false.
        unsafe {
            (*peer.file().as_raw()).master =
                bindings::drm_master_get((*owner.file().as_raw()).master);
        }
        let master = owner.file().associated_master().ok_or(EINVAL)?;
        let shared = peer.file().associated_master().ok_or(EINVAL)?;
        check(master == shared)?;
        let guard = master.lock_current().ok_or(EINVAL)?;
        check(guard.is_master_file(owner.file()))?;
        check(!guard.is_master_file(peer.file()))?;
        check(!guard.is_master_file(unassociated.file()))?;
        check(!guard.is_master_file(foreign.file()))?;
        Ok(())
    }

    #[test]
    fn master_file_checks_match_the_lease_identity_not_only_its_root() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-master-file-lease", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let owner = HandleClient::new(&dev)?;
        let client = HandleClient::new(&dev)?;
        make_current(&owner)?;
        associate_lessee(&client, &owner)?;
        // SAFETY: The private client owns its new lease identity, as a native lease file does.
        // Set its role before exposing it to the checks; native close revokes that lease.
        unsafe {
            (*client.file().as_raw()).is_master = true;
            (*client.file().as_raw()).was_master = true;
        }
        let root = owner.file().associated_master().ok_or(EINVAL)?;
        let lessee = client.file().associated_master().ok_or(EINVAL)?;
        {
            let guard = root.lock_current().ok_or(EINVAL)?;
            check(guard.master() == &root)?;
            check(guard.is_master_file(owner.file()))?;
            check(!guard.is_master_file(client.file()))?;
        }
        {
            let guard = lessee.lock_current().ok_or(EINVAL)?;
            check(guard.master() == &lessee)?;
            check(guard.is_master_file(client.file()))?;
            check(!guard.is_master_file(owner.file()))?;
        }
        Ok(())
    }

    #[test]
    fn lease_identity_is_independent_of_current_control() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-master-lease-identity", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let owner = HandleClient::new(&dev)?;
        let client = HandleClient::new(&dev)?;
        make_current(&owner)?;
        associate_lessee(&client, &owner)?;
        let root = owner.file().associated_master().ok_or(EINVAL)?;
        let lessee = client.file().associated_master().ok_or(EINVAL)?;
        check(!root.is_lessee())?;
        check(lessee.is_lessee())?;
        {
            // Even an empty lease has a current root. Identity does not imply object access.
            let _guard = lessee.lock_current().ok_or(EINVAL)?;
        }
        drop(owner);
        check(root.lock_current().is_none())?;
        check(lessee.lock_current().is_none())?;
        check(!root.is_lessee())?;
        check(lessee.is_lessee())?;
        drop(client);
        check(lessee.clone().is_lessee())?;
        drop(root);
        drop(lessee);
        drop(dev);
        check(counts.objects.load(Ordering::Relaxed) == 0)?;
        Ok(())
    }

    #[test]
    fn retained_identity_refuses_access_after_file_close() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-master-access-close", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let client = HandleClient::new(&dev)?;
        make_current(&client)?;
        let master = client.file().associated_master().ok_or(EINVAL)?;
        {
            let _guard = master.lock_current().ok_or(EINVAL)?;
        }
        drop(client);
        check(master.lock_current().is_none())?;
        drop(master);
        drop(dev);
        check(counts.objects.load(Ordering::Relaxed) == 0)?;
        Ok(())
    }

    #[test]
    fn object_access_rejects_another_device() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-master-access-devices", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let other = create(parent.as_ref(), &counts, false)?;
        let client = HandleClient::new(&dev)?;
        make_current(&client)?;
        let master = client.file().associated_master().ok_or(EINVAL)?;
        // SAFETY: Each initialized device owns its observed CRTC throughout the test.
        let crtc = unsafe { crtc::Crtc::<TestCrtc>::from_raw(dev.crtc.load(Ordering::Relaxed)) };
        // SAFETY: The independently retained second device owns this observed CRTC.
        let foreign =
            unsafe { crtc::Crtc::<TestCrtc>::from_raw(other.crtc.load(Ordering::Relaxed)) };
        let guard = master.lock_current().ok_or(EINVAL)?;
        check(guard.holds_object(crtc))?;
        check(!guard.holds_object(foreign))?;
        Ok(())
    }

    #[test]
    fn exclusive_object_access_rejects_descendant_leases() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-master-exclusive-access", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let owner = HandleClient::new(&dev)?;
        let lessee = HandleClient::new(&dev)?;
        make_current(&owner)?;
        associate_lessee(&lessee, &owner)?;
        // SAFETY: The initialized device owns this CRTC throughout the test.
        let crtc = unsafe { crtc::Crtc::<TestCrtc>::from_raw(dev.crtc.load(Ordering::Relaxed)) };
        // SAFETY: Both private files and their master identities remain live. The object-ID lock
        // serializes the synthetic lease update exactly like native lease construction.
        let id = unsafe { (*crtc.as_raw()).base.id };
        let raw_dev = owner.file().device_raw();
        let lease = unsafe { (*lessee.file().as_raw()).master };
        unsafe { bindings::mutex_lock(&raw mut (*raw_dev).mode_config.idr_mutex) };
        // SAFETY: The lease owns an initialized empty IDR and the registered CRTC stays live.
        let added = unsafe {
            bindings::idr_alloc(
                &raw mut (*lease).leases,
                crtc.as_raw().cast(),
                id as i32,
                id.saturating_add(1) as i32,
                GFP_KERNEL.as_raw(),
            )
        };
        unsafe { bindings::mutex_unlock(&raw mut (*raw_dev).mode_config.idr_mutex) };
        check(added == id as i32)?;
        let master = owner.file().associated_master().ok_or(EINVAL)?;
        {
            let guard = master.lock_current().ok_or(EINVAL)?;
            check(guard.holds_object(crtc))?;
            check(!guard.exclusively_holds_object(crtc))?;
        }
        // SAFETY: The same private lease and device remain live, and no guard holds the lock.
        unsafe {
            bindings::mutex_lock(&raw mut (*raw_dev).mode_config.idr_mutex);
            bindings::idr_remove(&raw mut (*lease).leases, id as usize);
            bindings::mutex_unlock(&raw mut (*raw_dev).mode_config.idr_mutex);
        }
        let guard = master.lock_current().ok_or(EINVAL)?;
        check(guard.exclusively_holds_object(crtc))
    }

    #[test]
    fn returning_an_error_releases_both_native_locks() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-master-access-error", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let client = HandleClient::new(&dev)?;
        make_current(&client)?;
        let master = client.file().associated_master().ok_or(EINVAL)?;
        let attempt = || -> Result {
            let _guard = master.lock_current().ok_or(EINVAL)?;
            Err(ECANCELED)
        };
        check(attempt() == Err(ECANCELED))?;
        let _guard = master.lock_current().ok_or(EINVAL)?;
        Ok(())
    }
}
