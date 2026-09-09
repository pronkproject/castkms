// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;

#[kunit_tests(rust_drm_master_transitions)]
mod tests {
    use super::*;

    #[test]
    fn installed_callback_observes_master_then_native_file_close() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-master-transition", None)?;
        let dev = create(parent.as_ref(), &counts, false)?;
        let client = HandleClient::new(&dev)?;
        associate(&client)?;
        let file = client.file();
        // SAFETY: The private initialized device has its immutable driver table installed.
        let callback = unsafe { (*(*dev.as_raw()).driver).master_set }.ok_or(EINVAL)?;
        // SAFETY: No other task uses the private device or file. Install a referenced master
        // under the native mutex and invoke the installed callback with its native lock
        // contract. No Rust state borrow or fallible operation overlaps the critical section.
        unsafe {
            bindings::mutex_lock(&raw mut (*dev.as_raw()).master_mutex);
            (*file.as_raw()).is_master = true;
            (*dev.as_raw()).master = bindings::drm_master_get((*file.as_raw()).master);
            callback(dev.as_raw(), file.as_raw(), true);
            bindings::mutex_unlock(&raw mut (*dev.as_raw()).master_mutex);
        }
        let snapshot = file.master_snapshot().ok_or(EINVAL)?;
        assert!(snapshot.was_current());
        assert_eq!(counts.master_sets.load(Ordering::Relaxed), 1);
        assert_eq!(counts.master_drops.load(Ordering::Relaxed), 0);
        // The ordinary internal-client close path must deliver the matching removal event.
        drop(client);
        assert_eq!(counts.master_drops.load(Ordering::Relaxed), 1);
        drop(snapshot);
        drop(dev);
        assert_eq!(counts.objects.load(Ordering::Relaxed), 0);
        Ok(())
    }
}
