// SPDX-License-Identifier: GPL-2.0-only

//! Display execution limits, independent of pixel access and capture permission.

pub(crate) mod host;
pub(crate) mod potential;
pub(crate) mod capabilities;
#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
pub(crate) mod constraints;
pub(crate) mod validation;
pub(crate) mod coordinator;

pub(crate) mod publication;
