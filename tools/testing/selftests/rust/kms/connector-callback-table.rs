// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0438\]
#![no_std]

use core::marker::PhantomData;
use kernel::{
    drm::{kms::connector::*, Device},
    prelude::*,
};

// A different layout from T makes another implementation's callbacks invalid.
pub struct Replacement<T: DriverConnector> {
    marker: PhantomData<fn() -> T>,
    payload: [u64; 8],
}

pub struct Payload<T: DriverConnector>(PhantomData<fn() -> T>);

impl<T: DriverConnector> Clone for Payload<T> {
    fn clone(&self) -> Self {
        Self(PhantomData)
    }
}

impl<T: DriverConnector> Default for Payload<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<T: DriverConnector> DriverConnectorState for Payload<T> {
    type Connector = Replacement<T>;
}

#[vtable]
impl<T: DriverConnector> DriverConnector for Replacement<T> {
    type OwnerModule = T::OwnerModule;
    type Driver = T::Driver;
    type Args = ();
    type State = Payload<T>;

    #[cfg(negative)]
    const OPS: &'static DriverConnectorOps = T::OPS;

    fn new(_: &Device<T::Driver>, _: ()) -> impl PinInit<Self, Error> {
        Ok(Self {
            marker: PhantomData,
            payload: [0; 8],
        })
    }

    fn get_modes<'a>(
        _: ConnectorGuard<'a, Self>,
        _: &kernel::drm::kms::ModeConfigGuard<'a, T::Driver>,
    ) -> i32 {
        0
    }
}
