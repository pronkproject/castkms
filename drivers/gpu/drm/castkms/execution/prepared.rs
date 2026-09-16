// SPDX-License-Identifier: GPL-2.0-only

//! One preallocated execution description, with retired storage returned after publication.

use super::{
    property,
    Description,
    Profile, //
};
use crate::Driver;
use kernel::{
    drm::{
        device::Registered,
        kms::blob::Blob,
        Device, //
    },
    prelude::*,
    sync::Arc, //
};

#[derive(Clone, Copy)]
pub(super) struct Change {
    pub(super) expected: Description,
    pub(super) next: Description,
}

/// Metadata prepared for one publication object, not permission to activate a renderer.
///
/// No source or image pool is retained. Successful publication exchanges the prepared blob
/// for the retired one and consumes the transition. Drop outside native object-ID and driver
/// control locks so retired native metadata is released only after control has finished.
pub(crate) struct Prepared {
    pub(super) origin: Arc<()>,
    pub(super) pending: Option<Change>,
    pub(super) blob: Blob<Driver>,
}

impl Prepared {
    pub(super) fn new(
        device: &Device<Driver, Registered>,
        origin: Arc<()>,
        expected: Description,
        profile: Profile,
    ) -> Result<Self> {
        let next = next_description(expected, profile)?;
        let blob = Blob::new(device, &property::encode(next))?;
        Ok(Self {
            origin,
            pending: Some(Change { expected, next }),
            blob,
        })
    }

    pub(crate) fn description(&self) -> Result<Description> {
        Ok(self.pending.ok_or(EALREADY)?.next)
    }
}

fn next_description(expected: Description, profile: Profile) -> Result<Description> {
    Ok(Description {
        profile,
        generation: expected.generation.checked_add(1).ok_or(EOVERFLOW)?,
    })
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_execution_generation)]
mod tests {
    use super::*;

    #[test]
    fn description_generations_never_wrap() {
        let first = super::super::initial();
        assert_eq!(
            next_description(first, Profile::HostV1),
            Ok(Description {
                generation: 2,
                ..first
            })
        );
        let exhausted = Description {
            generation: u64::MAX,
            ..first
        };
        assert_eq!(next_description(exhausted, Profile::HostV1), Err(EOVERFLOW));
    }
}
