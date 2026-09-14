// SPDX-License-Identifier: GPL-2.0-only

//! Display execution limits, independent of pixel access and capture permission.

pub(crate) mod host;

pub(crate) mod property;
pub(crate) mod publication;
mod prepared;
pub(crate) use prepared::Prepared;

/// The initial renderer profile, before delegated execution is available.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Profile {
    HostV1,
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
