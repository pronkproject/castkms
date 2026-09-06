// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0596\]
#![no_std]

use kernel::drm::{
    device::{Device, Registered},
    kms::KmsDriver,
};
use kernel::prelude::*;

pub fn nested<D: KmsDriver>(
    first: &Device<D, Registered>,
    second: &Device<D, Registered>,
) -> Result {
    first.check_atomic_update(|mut outer| {
        second.check_atomic_update(|mut inner| {
            #[cfg(negative)]
            core::mem::swap(&mut *outer, &mut *inner);
            let _ = (&mut outer, &mut inner);
            Ok(())
        })
    })
}
