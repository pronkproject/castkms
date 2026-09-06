// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: cannot be (sent|shared) between threads safely
#![no_std]

use kernel::drm::{Driver, Registration};

fn send_sync<T: Send + Sync>(_: &T) {}

// Do not require Send or Sync on D: the registration's fields and Driver's associated-data
// bounds, not blanket unsafe implementations or extra driver restrictions, establish sharing.
pub fn owned<D: Driver>(registration: &Registration<'_, D>) {
    send_sync(registration);
    #[cfg(negative)]
    if let Some(guard) = registration.registration_guard() {
        send_sync(&guard);
    }
}
