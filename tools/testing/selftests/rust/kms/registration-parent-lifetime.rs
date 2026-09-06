// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: call to unsafe function.*new_static.*is unsafe
#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

use kernel::{device, drm, prelude::*};

// Static payloads alone do not establish parent-binding lifetime. The unsafe caller must own
// teardown ordering; the negative case attempts the same construction without that proof.
pub unsafe fn owned<D: drm::Driver>(
    parent: &device::Device<device::Bound>,
    device: drm::UnregisteredDevice<D>,
    payload: D::RegistrationData<'static>,
) -> Result<drm::Registration<'static, D>> {
    #[cfg(not(negative))]
    unsafe {
        drm::Registration::new_static(parent, device, Ok::<_, Error>(payload), 0)
    }
    #[cfg(negative)]
    {
        // The fixture itself is unsafe only to document the positive caller's obligation.
        // Deny the implicit unsafe block so omission of an explicit block is an error.
        drm::Registration::new_static(parent, device, Ok::<_, Error>(payload), 0)
    }
}
