// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Native master transition adapters, called with DRM's master mutex held.

use super::*;

pub(crate) unsafe extern "C" fn master_set<D: Driver>(
    raw_dev: *mut bindings::drm_device,
    file: *mut bindings::drm_file,
    _from_open: bool,
) {
    // SAFETY: DRM invokes the installed driver callback with its live device and file.
    let dev = unsafe { Device::<D>::from_raw(raw_dev) };
    // SAFETY: Native master lookup uses the file's spinlock, not the held master mutex.
    // The callback can precede the driver's file-open callback, so no typed file is made.
    let master = NonNull::new(unsafe { bindings::drm_file_get_master(file) });
    let master = master.map(|raw| {
        // SAFETY: Lookup returned one reference to a master on this callback's device.
        unsafe { MasterRef::from_owned_raw(raw, dev) }
    });
    D::master_changed(dev, master);
}

pub(crate) unsafe extern "C" fn master_drop<D: Driver>(
    raw_dev: *mut bindings::drm_device,
    _file: *mut bindings::drm_file,
) {
    // SAFETY: DRM retains the device throughout its installed callback.
    let dev = unsafe { Device::<D>::from_raw(raw_dev) };
    D::master_changed(dev, None);
}
