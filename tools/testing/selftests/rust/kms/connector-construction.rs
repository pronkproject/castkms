// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0271\]
#![no_std]

use kernel::drm::kms::{connector::*, KmsDriver, UnregisteredKmsDevice};

pub fn nominated<'a, T: DriverConnector>(
    dev: &'a UnregisteredKmsDevice<'a, T::Driver>,
    kind: Type,
    args: T::Args,
) where
    T::Driver: KmsDriver<Connector = T>,
{
    let _ = UnregisteredConnector::<T>::new(dev, kind, args);
}

#[cfg(negative)]
pub fn not_nominated<'a, T: DriverConnector>(
    dev: &'a UnregisteredKmsDevice<'a, T::Driver>,
    kind: Type,
    args: T::Args,
) {
    let _ = UnregisteredConnector::<T>::new(dev, kind, args);
}
