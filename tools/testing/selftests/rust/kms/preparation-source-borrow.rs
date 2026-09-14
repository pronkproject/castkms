// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: lifetime may not live long enough
#![no_std]

use kernel::{
    drm::{
        device::{
            Device,
            Registered, //
        },
        kms::crtc::{
            Crtc,
            DriverCrtc, //
        },
        preparation::Source, //
    },
    prelude::*,
    sync::aref::ARef, //
};

pub fn observe<C: DriverCrtc>(
    device: &Device<C::Driver, Registered>,
    crtc: &Crtc<C>,
) -> Result<Option<ARef<Source>>> {
    #[cfg(negative)]
    let borrowed = device.with_crtc_preparation_source(crtc, |source| source)?;
    #[cfg(negative)]
    return Ok(borrowed.map(Into::into));

    #[cfg(not(negative))]
    device.with_crtc_preparation_source(crtc, |source| source.map(Into::into))
}
