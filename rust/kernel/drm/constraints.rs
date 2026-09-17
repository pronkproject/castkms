// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Immutable DRM constraints descriptions and retained backend bindings.
//!
//! Allocation descriptions guide buffer construction; they do not authorize display or source
//! access. Native DRM validates descriptions and owns their reference-counted storage. Provider
//! atomic checks remain responsible for complete scenes and resource combinations.

mod description;
mod entry;
mod list;
mod property;

pub use property::Property;

pub use description::{
    Description,
    Format,
    PlaneLimit,
    Size, //
};

pub use entry::{
    Backend,
    Domain,
    Entry,
    OpaqueEntry, //
};

pub use list::{
    List,
    Offer,
    Snapshot,
    SnapshotInfo, //
};
