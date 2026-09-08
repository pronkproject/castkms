// SPDX-License-Identifier: GPL-2.0-only

//! Local display buffer allocation. No compositor mapping or capture authority lives here.

use super::Driver;
use kernel::{
    drm,
    prelude::*, //
};

#[pin_data]
pub(super) struct Object {}

#[vtable]
impl drm::gem::DriverObject for Object {
    type Driver = Driver;
    type Args = ();

    fn new(_: &drm::Device<Driver>, _: usize, _: ()) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {})
    }
}
