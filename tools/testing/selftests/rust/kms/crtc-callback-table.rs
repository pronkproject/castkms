// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0438\]
#![no_std]

use core::marker::PhantomData;
use kernel::{
    drm::{kms::crtc::*, Device},
    prelude::*,
};

// A different layout from T makes another implementation's callbacks invalid.
pub struct Replacement<T: DriverCrtc> {
    marker: PhantomData<fn() -> T>,
    payload: [u64; 8],
}

pub struct Payload<T: DriverCrtc>(PhantomData<fn() -> T>);

impl<T: DriverCrtc> Clone for Payload<T> {
    fn clone(&self) -> Self {
        Self(PhantomData)
    }
}

impl<T: DriverCrtc> Default for Payload<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<T: DriverCrtc> DriverCrtcState for Payload<T> {
    type Crtc = Replacement<T>;

    fn new(_: &Crtc<Self::Crtc>) -> Result<Self> {
        Ok(Self(PhantomData))
    }

    fn duplicate(&self) -> Result<Self> {
        Ok(Self(PhantomData))
    }
}

#[vtable]
impl<T: DriverCrtc> DriverCrtc for Replacement<T> {
    type OwnerModule = T::OwnerModule;
    type Driver = T::Driver;
    type Args = ();
    type State = Payload<T>;
    type VblankImpl = PhantomData<Self>;

    #[cfg(negative)]
    const OPS: &'static DriverCrtcOps = T::OPS;

    fn new(_: &Device<T::Driver>, _: &()) -> impl PinInit<Self, Error> {
        Ok(Self {
            marker: PhantomData,
            payload: [0; 8],
        })
    }
}
