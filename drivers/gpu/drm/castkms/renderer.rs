// SPDX-License-Identifier: GPL-2.0-only

//! Separately authorized delegated rendering, independent of capture grant transport.

pub(crate) mod candidate;
pub(crate) mod description;
mod client_file;
pub(crate) mod files;
pub(crate) mod job;
pub(crate) mod permission;
mod probe;
mod revoker_file;
mod scene_file;
pub(crate) mod session;
