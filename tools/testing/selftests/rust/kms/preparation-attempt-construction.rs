// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: field `raw` of struct `Attempt` is private
#![no_std]

use kernel::{
    drm::preparation::{Attempt, Ticket},
    error::Result,
};

pub fn reserve(ticket: &Ticket) -> Result<Attempt> {
    #[cfg(negative)]
    return Ok(Attempt { raw: core::ptr::NonNull::dangling() });
    #[cfg(not(negative))]
    ticket.reserve()
}
