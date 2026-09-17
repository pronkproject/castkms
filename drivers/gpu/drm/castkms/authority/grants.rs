// SPDX-License-Identifier: GPL-2.0-only

//! Device-wide revocation tracking shared by independently authorized providers.

use kernel::{
    drm::capture::Revocation,
    prelude::*,
    sync::{
        aref::ARef,
        Arc,
        Completion,
        Mutex, //
    }, //
};

const MAX_GRANTS: usize = 256;
const MAX_DEVICE_WORKERS: usize = 512;

struct Entry {
    revocation: ARef<Revocation>,
}

struct State {
    closed: bool,
    grants: KVec<Arc<Entry>>,
}

#[pin_data]
pub(crate) struct Registry {
    capacity: usize,
    #[pin]
    state: Mutex<State>,
    #[pin]
    cleanup_done: Completion,
}

impl Registry {
    pub(crate) fn new() -> Result<Arc<Self>> {
        Self::new_with_capacity(MAX_GRANTS)
    }

    pub(crate) fn new_device_workers() -> Result<Arc<Self>> {
        Self::new_with_capacity(MAX_DEVICE_WORKERS)
    }

    fn new_with_capacity(capacity: usize) -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                capacity,
                state <- kernel::new_mutex!(State { closed: false, grants: KVec::new() }),
                cleanup_done <- Completion::new(),
            }),
            GFP_KERNEL,
        )
    }

    /// Track a grant until device shutdown or its unique registration owner closes.
    ///
    /// Tracking adds no resource permission. The provider must retain the returned owner
    /// outside the device registry, so its destruction breaks retained device references.
    pub(crate) fn register(self: &Arc<Self>, revocation: &Revocation) -> Result<Registration> {
        let entry = Arc::new(
            Entry {
                revocation: revocation.into(),
            },
            GFP_KERNEL,
        )?;
        {
            let mut state = self.state.lock();
            if state.closed {
                return Err(ENODEV);
            }
            if state
                .grants
                .iter()
                .any(|entry| core::ptr::eq(&*entry.revocation, revocation))
            {
                return Err(EEXIST);
            }
            if state.grants.len() == self.capacity {
                return Err(EBUSY);
            }
            state.grants.push(entry.clone(), GFP_KERNEL)?;
        }
        Ok(Registration {
            registry: self.clone(),
            entry,
        })
    }

    /// Revoke every tracked work generation without closing later registration.
    pub(crate) fn revoke_all(&self) {
        loop {
            let next = self
                .state
                .lock()
                .grants
                .iter()
                .find(|entry| !entry.revocation.is_revoked())
                .cloned();
            let Some(entry) = next else { break };
            entry.revocation.revoke();
        }
    }

    /// Permanently stop registration and finish revoking all previously tracked grants.
    ///
    /// Concurrent calls wait for the first cleanup pass. Hold no provider cleanup locks;
    /// a provider callback must not recursively close the same registry.
    pub(crate) fn close(&self) {
        let retired = {
            let mut state = self.state.lock();
            if state.closed {
                None
            } else {
                state.closed = true;
                Some(core::mem::take(&mut state.grants))
            }
        };
        if let Some(retired) = retired {
            for entry in &retired {
                entry.revocation.revoke();
            }
            drop(retired);
            self.cleanup_done.complete_all();
        } else {
            self.cleanup_done.wait_for_completion();
        }
    }

    fn remove(&self, entry: &Arc<Entry>) {
        let retired = {
            let mut state = self.state.lock();
            let index = state
                .grants
                .iter()
                .position(|current| Arc::ptr_eq(current, entry));
            index.and_then(|index| state.grants.remove(index).ok())
        };
        drop(retired);
    }
}

/// Unique removal owner. The registry retains neither this token nor its containing grantor.
///
/// Drop revokes before removing tracking and may wait for provider cleanup. Hold no DRM,
/// admission or provider cleanup locks when releasing the token.
#[must_use = "dropping the registration revokes the grant"]
pub(crate) struct Registration {
    registry: Arc<Registry>,
    entry: Arc<Entry>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.entry.revocation.revoke();
        self.registry.remove(&self.entry);
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
