// SPDX-License-Identifier: GPL-2.0-only

//! Display storage allocation and import, without compositor mapping or capture authority.

use super::Driver;
use kernel::{
    drm,
    prelude::*,
    sync::Arc, //
};

pub(crate) mod budget;

/// Optional allocation credit, supplied only by the checked storage factory.
#[derive(Default)]
pub(super) struct Args {
    charge: Option<budget::Charge>,
}

#[pin_data]
pub(super) struct Object {
    // The shmem helper releases native storage before dropping the private payload.
    _charge: Option<budget::Charge>,
}

impl Object {
    /// Allocate private storage whose final native reference returns its budget.
    #[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
    pub(crate) fn new_budgeted(
        device: &drm::Device<Driver>,
        budget: &Arc<budget::Budget>,
        size: usize,
    ) -> Result<drm::gem::ObjectRef<drm::gem::shmem::Object<Self>>> {
        let charge = budget.reserve(size)?;
        drm::gem::shmem::Object::new(
            device,
            size,
            Default::default(),
            Args {
                charge: Some(charge),
            },
        )
    }
}

#[vtable]
impl drm::gem::DriverObject for Object {
    type Driver = Driver;
    type Args = Args;

    fn new(_: &drm::Device<Driver>, size: usize, args: Args) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            _charge: {
                if args
                    .charge
                    .as_ref()
                    .is_some_and(|charge| charge.bytes != size)
                {
                    return Err(EINVAL);
                }
                args.charge
            },
        })
    }

    fn dumb_create_args(_: &drm::Device<Driver>, _: usize) -> Result<Args> {
        Ok(Args::default())
    }

    fn prime_import_args(_: &drm::Device<Driver>, _: usize) -> Result<Args> {
        Ok(Args::default())
    }
}
