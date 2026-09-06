// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0271\]
#![no_std]

use kernel::drm::kms::{plane::*, KmsDriver, UnregisteredKmsDevice};

pub fn nominated<'a, T: DriverPlane>(dev: &'a UnregisteredKmsDevice<'a, T::Driver>, args: T::Args)
where
    T::Driver: KmsDriver<Plane = T>,
{
    let _ = UnregisteredPlane::<T>::new(dev, 0, &[], None, Type::Primary, None, args);
}

#[cfg(negative)]
pub fn not_nominated<'a, T: DriverPlane>(
    dev: &'a UnregisteredKmsDevice<'a, T::Driver>,
    args: T::Args,
) {
    let _ = UnregisteredPlane::<T>::new(dev, 0, &[], None, Type::Primary, None, args);
}
