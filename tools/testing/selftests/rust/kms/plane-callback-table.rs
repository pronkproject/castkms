// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0438\]
#![no_std]

use core::marker::PhantomData;
use kernel::{
    drm::{kms::plane::*, Device},
    prelude::*,
};

// A different layout from T makes another implementation's callbacks invalid.
pub struct Replacement<T: DriverPlane> {
    marker: PhantomData<fn() -> T>,
    payload: [u64; 8],
}

pub struct Payload<T: DriverPlane>(PhantomData<fn() -> T>);

impl<T: DriverPlane> Clone for Payload<T> {
    fn clone(&self) -> Self {
        Self(PhantomData)
    }
}

impl<T: DriverPlane> Default for Payload<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<T: DriverPlane> DriverPlaneState for Payload<T> {
    type Plane = Replacement<T>;

    fn new(_: &Plane<Self::Plane>) -> Result<Self> {
        Ok(Self(PhantomData))
    }

    fn duplicate(&self) -> Result<Self> {
        Ok(Self(PhantomData))
    }
}

#[vtable]
impl<T: DriverPlane> DriverPlane for Replacement<T> {
    type OwnerModule = T::OwnerModule;
    type Driver = T::Driver;
    type Args = ();
    type State = Payload<T>;

    #[cfg(negative)]
    const OPS: &'static DriverPlaneOps = T::OPS;

    fn new(_: &Device<T::Driver>, _: ()) -> impl PinInit<Self, Error> {
        Ok(Self {
            marker: PhantomData,
            payload: [0; 8],
        })
    }
}
