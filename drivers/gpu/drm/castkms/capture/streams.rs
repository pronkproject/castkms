// SPDX-License-Identifier: GPL-2.0-only

//! Streams revoked when display control changes, without retaining grant callbacks.
//!
//! Registration tracks delivery lifetime, not pixel permission. The provider must establish
//! current access while excluding master transitions before registering a stream.

use kernel::{
    drm::capture::Stream,
    prelude::*,
    sync::{
        aref::ARef,
        Arc,
        Mutex, //
    }, //
};

struct State {
    closed: bool,
    streams: KVec<Arc<Entry>>,
}

/// Identifies one registration independently of the native stream it retains.
struct Entry {
    stream: ARef<Stream>,
}

#[pin_data]
pub(crate) struct Registry {
    #[pin]
    state: Mutex<State>,
}

impl Registry {
    pub(crate) fn new() -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                state <- kernel::new_mutex!(State { closed: false, streams: KVec::new() }),
            }),
            GFP_KERNEL,
        )
    }

    /// Keep a stream in the current control interval until its unique registration is dropped.
    #[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
    pub(crate) fn register(self: &Arc<Self>, stream: &Stream) -> Result<Registration> {
        let entry = Arc::new(
            Entry {
                stream: stream.into(),
            },
            GFP_KERNEL,
        )?;
        {
            let mut state = self.state.lock();
            if state.closed {
                return Err(ENODEV);
            }
            if state
                .streams
                .iter()
                .any(|entry| core::ptr::eq(&*entry.stream, stream))
            {
                return Err(EEXIST);
            }
            // The local record keeps allocation failure from releasing its stream under this lock.
            state.streams.push(entry.clone(), GFP_KERNEL)?;
        }
        Ok(Registration {
            registry: self.clone(),
            entry,
        })
    }

    fn revoke_locked(state: &mut State) -> KVec<Arc<Entry>> {
        // Native revocation only records errors and wakes observers. It does not invoke
        // provider callbacks or release storage. Serialize it with registration and removal
        // so every returning close observes the completed revocation of earlier streams.
        for entry in &state.streams {
            entry.stream.revoke();
        }
        core::mem::take(&mut state.streams)
    }

    /// End every old stream; a later control interval may register new streams.
    pub(crate) fn revoke_all(&self) {
        let retired = Self::revoke_locked(&mut self.state.lock());
        drop(retired);
    }

    /// Permanently stop registration and revoke streams even if their handles remain alive.
    pub(crate) fn close(&self) {
        let retired = {
            let mut state = self.state.lock();
            state.closed = true;
            Self::revoke_locked(&mut state)
        };
        drop(retired);
    }

    fn remove(&self, entry: &Arc<Entry>) {
        let retired = {
            let mut state = self.state.lock();
            let index = state
                .streams
                .iter()
                .position(|current| Arc::ptr_eq(current, entry));
            index.and_then(|index| {
                entry.stream.revoke();
                state.streams.remove(index).ok()
            })
        };
        // A final native reference may destroy queued results; release it after unlocking.
        drop(retired);
    }
}

/// One stream's registration. It retains the registry; the registry never retains this handle.
#[must_use = "dropping the registration revokes its stream"]
pub(crate) struct Registration {
    registry: Arc<Registry>,
    entry: Arc<Entry>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.registry.remove(&self.entry);
    }
}
