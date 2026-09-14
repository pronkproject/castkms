// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: cannot move out of .buffer. because it is borrowed
#![no_std]

use kernel::{
    dma_buf::DmaBuf,
    drm::{
        capture::{Destination, DestinationPlane},
        fourcc, //
    },
    prelude::*,
    sync::aref::ARef, //
};

pub fn dimensions(buffer: ARef<DmaBuf>) -> Result<[u32; 2]> {
    let destination = Destination::new(
        [16, 16],
        fourcc::XRGB8888,
        fourcc::FORMAT_MOD_LINEAR,
        &[DestinationPlane::new(&buffer, 64, 0)],
    )?;
    #[cfg(negative)]
    drop(buffer);
    let dimensions = destination.dimensions();
    #[cfg(not(negative))]
    drop(buffer);
    Ok(dimensions)
}
