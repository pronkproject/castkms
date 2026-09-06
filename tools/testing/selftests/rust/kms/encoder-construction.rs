// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0271\]
#![no_std]

use kernel::drm::kms::{encoder::*, KmsDriver, UnregisteredKmsDevice};

pub fn nominated<'a, T: DriverEncoder>(
    dev: &'a UnregisteredKmsDevice<'a, T::Driver>,
    kind: Type,
    args: T::Args,
) where
    T::Driver: KmsDriver<Encoder = T>,
{
    let _ = UnregisteredEncoder::<T>::new(dev, kind, 0, 0, None, args);
}

#[cfg(negative)]
pub fn not_nominated<'a, T: DriverEncoder>(
    dev: &'a UnregisteredKmsDevice<'a, T::Driver>,
    kind: Type,
    args: T::Args,
) {
    let _ = UnregisteredEncoder::<T>::new(dev, kind, 0, 0, None, args);
}
