// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: cannot move out of `job` because it is borrowed
#![no_std]

use kernel::drm::capture::Job;

pub fn complete(mut job: Job) {
    let pixels = job.data_mut();
    #[cfg(negative)]
    job.complete(Ok(()));
    pixels.fill(0x55);
    #[cfg(not(negative))]
    job.complete(Ok(()));
}
