// SPDX-License-Identifier: GPL-2.0-only

//! Display storage allocation and import, without compositor mapping or capture authority.

use super::Driver;
use kernel::{drm, prelude::*};

#[derive(Default)]
pub(super) struct Args;

#[pin_data]
pub(super) struct Object;

#[vtable]
impl drm::gem::DriverObject for Object {
    type Driver = Driver;
    type Args = Args;

    fn new(_: &drm::Device<Driver>, _: usize, _: Args) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {})
    }

    fn dumb_create_args(_: &drm::Device<Driver>, _: usize) -> Result<Args> {
        Ok(Args::default())
    }

    fn prime_import_args(_: &drm::Device<Driver>, _: usize) -> Result<Args> {
        Ok(Args::default())
    }
}
