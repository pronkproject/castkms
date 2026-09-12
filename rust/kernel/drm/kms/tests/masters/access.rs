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
