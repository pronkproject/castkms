// SPDX-License-Identifier: GPL-2.0-only

//! Lifetime exclusion for the in-kernel compositor.

use kernel::{prelude::*, sync::{Mutex, MutexGuard}};

pub(crate) struct HostAdmission<'a> {
    _state: MutexGuard<'a, bool>,
}

#[pin_data]
pub(crate) struct Publication {
    #[pin]
    closed: Mutex<bool>,
}

impl Publication {
    pub(crate) fn new() -> impl PinInit<Self, Error> {
        try_pin_init!(Self { closed <- kernel::new_mutex!(false) })
    }

    pub(crate) fn admit_host(&self) -> Result<HostAdmission<'_>> {
        let state = self.closed.lock();
        if *state { return Err(ENODEV); }
        Ok(HostAdmission { _state: state })
    }

    pub(crate) fn check_host(&self) -> Result {
        drop(self.admit_host()?);
        Ok(())
    }

    pub(crate) fn close(&self) {
        *self.closed.lock() = true;
    }
}
