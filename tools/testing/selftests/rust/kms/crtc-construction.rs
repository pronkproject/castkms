// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0271\]
#![no_std]

use kernel::drm::kms::{crtc::*, plane::*, KmsDriver, UnregisteredKmsDevice};

pub fn nominated<'a, T: DriverCrtc>(
    dev: &'a UnregisteredKmsDevice<'a, T::Driver>,
    primary: &'a UnregisteredPlane<<T::Driver as KmsDriver>::Plane>,
    args: T::Args,
) where
    T::Driver: KmsDriver<Crtc = T>,
{
    let _ = UnregisteredCrtc::<T>::new(
        dev,
        primary,
        None::<&UnregisteredPlane<<T::Driver as KmsDriver>::Plane>>,
        None,
        args,
    );
}

#[cfg(negative)]
pub fn not_nominated<'a, T: DriverCrtc>(
    dev: &'a UnregisteredKmsDevice<'a, T::Driver>,
    primary: &'a UnregisteredPlane<<T::Driver as KmsDriver>::Plane>,
    args: T::Args,
) {
    let _ = UnregisteredCrtc::<T>::new(
        dev,
        primary,
        None::<&UnregisteredPlane<<T::Driver as KmsDriver>::Plane>>,
        None,
        args,
    );
}
