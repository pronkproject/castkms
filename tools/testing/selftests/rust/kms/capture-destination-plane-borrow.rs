// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: cannot move out of .destination. because it is borrowed
#![no_std]

use kernel::drm::capture::Destination;

pub fn stride(destination: Destination<'_>) -> Option<u32> {
    let plane = destination.plane(0)?;
    #[cfg(negative)]
    drop(destination);
    let stride = plane.stride();
    #[cfg(not(negative))]
    drop(destination);
    Some(stride)
}
