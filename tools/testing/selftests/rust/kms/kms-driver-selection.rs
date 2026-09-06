// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0284\]
#![no_std]

use kernel::drm::{kms::KmsDriver, Driver};

pub fn selects_kms<D: KmsDriver>() {
    fn selected<D: Driver<Kms = D>>() {}
    selected::<D>();
}

#[cfg(negative)]
pub fn disabled<D>()
where
    D: KmsDriver,
    D: Driver<Kms = core::marker::PhantomData<D>>,
{
}
