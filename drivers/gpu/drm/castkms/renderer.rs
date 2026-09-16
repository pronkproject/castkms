// SPDX-License-Identifier: GPL-2.0-only

//! Separately authorized delegated rendering, independent of capture grant transport.

pub(crate) mod candidate;
pub(crate) mod content;
mod capability_description;
mod capability_file;
mod profile_file;
pub(crate) mod description;
mod client_file;
pub(crate) mod files;
pub(crate) mod job;
pub(crate) mod permission;
pub(crate) mod private_image;
pub(crate) mod private_pool;
mod probe;
pub(crate) mod proposal;
pub(crate) mod render_job;
mod revoker_file;
mod scene_file;
pub(crate) mod session;
