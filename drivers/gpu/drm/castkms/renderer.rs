// SPDX-License-Identifier: GPL-2.0-only

//! Separately authorized delegated rendering, independent of capture grant transport.

pub(crate) mod candidate;
mod client_file;
pub(crate) mod files;
pub(crate) mod permission;
mod probe;
mod revoker_file;
mod session;
