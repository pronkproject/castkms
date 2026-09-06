// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: lifetime may not live long enough
#![no_std]

use kernel::{device, drm, prelude::*};

pub fn scoped<D: drm::Driver>(
    parent: &device::Device<device::Bound>,
    device: drm::UnregisteredDevice<D>,
    payload: D::RegistrationData<'static>,
) -> Result {
    drm::Registration::with_static(parent, device, Ok::<_, Error>(payload), 0, |registration| {
        let guard = registration.registration_guard().ok_or(ENODEV)?;
        #[cfg(not(negative))]
        {
            drop(guard);
            Ok(())
        }
        #[cfg(negative)]
        {
            Ok(guard)
        }
    })?;
    Ok(())
}
