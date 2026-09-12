// SPDX-License-Identifier: GPL-2.0-only

//! Capture delivery adapters, above private rendering and the shared capture machinery.
//!
//! Delivery operations do not mint authority. Their caller must establish the recipient's
//! current permission and the image's authorized scope before providing either input.

pub(crate) mod host;
