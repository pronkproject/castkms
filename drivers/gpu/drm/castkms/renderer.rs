// SPDX-License-Identifier: GPL-2.0-only

//! Separately authorized delegated rendering, independent of capture grant transport.

pub(crate) mod candidate;
#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
pub(crate) mod draft;
#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
pub(crate) mod offer;
#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
pub(crate) mod endpoint;
pub(crate) mod content;
mod capability_description;
pub(crate) mod description;
mod client_file;
pub(crate) mod files;
pub(crate) mod job;
pub(crate) mod permission;
pub(crate) mod private_image;
pub(crate) mod private_pool;
pub(crate) mod ready;
mod probe;
pub(crate) mod proposal;
pub(crate) mod render_job;
pub(crate) mod routing;
pub(crate) mod output_broker;
mod revoker_file;
mod scene_file;
mod image_file;
pub(crate) mod session;
