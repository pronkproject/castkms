// SPDX-License-Identifier: GPL-2.0-only

//! Revocation when a grant's creating owner closes, independent of capture handles.

use super::Policy;
use kernel::{
    drm::capture::Authority,
    prelude::*,
    sync::{
        aref::ARef,
        Arc,
        Mutex, //
    }, //
};

const MAX_GRANTS: usize = 64;

struct Entry {
    authority: ARef<Authority<Policy>>,
}

#[pin_data]
struct Registry {
    #[pin]
    grants: Mutex<KVec<Arc<Entry>>>,
}

impl Registry {
    fn remove(&self, entry: &Arc<Entry>) {
        let retired = {
            let mut grants = self.grants.lock();
            let index = grants
                .iter()
                .position(|current| Arc::ptr_eq(current, entry));
            index.and_then(|index| grants.remove(index).ok())
        };
        drop(retired);
    }

    fn close(&self) {
        let retired = core::mem::take(&mut *self.grants.lock());
        // Provider callbacks and final policy references stay outside the tracking lock.
        for entry in &retired {
            entry.authority.revoke();
        }
    }
}

/// Unique owner of a bounded collection of grants, with no dependency on DRM files.
///
/// Dropping this owner revokes every still-tracked grant. Registering a grant borrows the
/// unique owner, so that operation cannot race its destruction. Surviving grantors retain
/// only tracking storage, not the owner or its file. Capture handles retain neither.
/// Drop may sleep and release final policy references; call outside DRM locks.
pub(crate) struct Creator(Arc<Registry>);

impl Creator {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self(Arc::pin_init(
            pin_init!(Registry {
                grants <- kernel::new_mutex!(KVec::new()),
            }),
            GFP_KERNEL,
        )?))
    }

    pub(super) fn register(&self, authority: &Authority<Policy>) -> Result<Registration> {
        let entry = Arc::new(
            Entry {
                authority: authority.into(),
            },
            GFP_KERNEL,
        )?;
        {
            let mut grants = self.0.grants.lock();
            if grants.len() == MAX_GRANTS {
                return Err(EBUSY);
            }
            grants.push(entry.clone(), GFP_KERNEL)?;
        }
        Ok(Registration {
            registry: self.0.clone(),
            entry,
        })
    }
}

impl Drop for Creator {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// The grantor revokes before releasing its registration; the registry owns no grantor.
pub(super) struct Registration {
    registry: Arc<Registry>,
    entry: Arc<Entry>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.registry.remove(&self.entry);
    }
}
