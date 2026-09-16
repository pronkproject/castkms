// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Immutable DRM constraints descriptions and retained backend bindings.
//!
//! Allocation descriptions guide buffer construction; they do not authorize display or source
//! access. Native DRM validates descriptions and owns their reference-counted storage. Provider
//! atomic checks remain responsible for complete scenes and resource combinations.

mod catalog;
mod description;
mod entry;

pub use description::{
    Description,
    Format,
    Size, //
};

pub use entry::{
    Backend,
    Domain,
    Entry, //
};

pub use catalog::{
    Catalog,
    Offer,
    Snapshot,
    SnapshotInfo, //
};
