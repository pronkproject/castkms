// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: use of moved value: `job`
#![no_std]

use kernel::drm::capture::Job;

pub fn complete(job: Job) {
    job.complete(Ok(()));
    #[cfg(negative)]
    job.complete(Ok(()));
}
