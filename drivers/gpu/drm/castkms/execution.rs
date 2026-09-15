// SPDX-License-Identifier: GPL-2.0-only

//! Display execution limits, independent of pixel access and capture permission.

pub(crate) mod host;
pub(crate) mod potential;
pub(crate) mod capabilities;
pub(crate) mod proposal;
pub(crate) mod validation;
pub(crate) mod coordinator;

pub(crate) mod property;
pub(crate) mod publication;
mod prepared;
pub(crate) use prepared::Prepared;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Profile {
    /// In-kernel composition over the restricted HOST framebuffer profile.
    HostV1,
    /// An activated userspace renderer owns display execution.
    GpuV1,
}

/// One coherent observation; it neither reserves execution nor authorizes capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Description {
    pub(crate) profile: Profile,
    pub(crate) generation: u64,
}

const fn initial() -> Description {
    Description {
        profile: Profile::HostV1,
        generation: 1,
    }
}
