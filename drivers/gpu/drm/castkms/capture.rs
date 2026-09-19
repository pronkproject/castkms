// SPDX-License-Identifier: GPL-2.0-only

//! Capture delivery adapters, above private rendering and the shared capture machinery.
//!
//! Delivery operations do not mint authority. Their caller must establish the recipient's
//! current permission and the image's authorized scope before providing either input.

pub(crate) mod budget;
pub(crate) mod client;
mod client_queue;
pub(crate) mod destination;
pub(crate) mod host;
pub(crate) mod host_stream;
pub(crate) mod host_queue;
pub(crate) mod negotiation;
mod output_layout;
pub(crate) mod permission;
pub(crate) mod provider;
pub(crate) mod requests;
pub(crate) mod request_budget;
mod reuse;
pub(crate) mod streams;
