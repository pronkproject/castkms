// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0438\]
#![no_std]

use core::marker::PhantomData;
use kernel::{
    drm::{kms::encoder::*, Device},
    prelude::*,
};

// A different layout from T makes another implementation's callbacks invalid.
pub struct Replacement<T: DriverEncoder> {
    marker: PhantomData<fn() -> T>,
    payload: [u64; 8],
}

#[vtable]
impl<T: DriverEncoder> DriverEncoder for Replacement<T> {
    type OwnerModule = T::OwnerModule;
    type Driver = T::Driver;
    type Args = ();

    #[cfg(negative)]
    const OPS: &'static DriverEncoderOps = T::OPS;

    fn new(_: &Device<T::Driver>, _: ()) -> impl PinInit<Self, Error> {
        Ok(Self {
            marker: PhantomData,
            payload: [0; 8],
        })
    }
}
