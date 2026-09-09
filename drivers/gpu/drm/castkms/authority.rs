// SPDX-License-Identifier: GPL-2.0-only

//! Top-level master observations, separate from scene ownership and lease authorization.

use kernel::{
    prelude::*,
    sync::Mutex, //
};

enum State<I> {
    Tracking(Option<I>),
    Closed,
}

#[pin_data]
pub(super) struct Authority<I> {
    #[pin]
    state: Mutex<State<I>>,
}

impl<I: Unpin> Authority<I> {
    pub(super) fn new() -> impl PinInit<Self> {
        pin_init!(Self { state <- kernel::new_mutex!(State::Tracking(None)) })
    }

    /// Called in native master transition order. Closing permanently rejects later events.
    pub(super) fn changed(&self, master: Option<I>) {
        let retired = {
            let mut state = self.state.lock();
            match &mut *state {
                State::Tracking(current) => core::mem::replace(current, master),
                State::Closed => master,
            }
        };
        drop(retired);
    }

    /// A coherent historical observation, never a permission to read pixels.
    pub(super) fn snapshot(&self) -> Option<I>
    where
        I: Clone,
    {
        match &*self.state.lock() {
            State::Tracking(current) => current.clone(),
            State::Closed => None,
        }
    }

    pub(super) fn close(&self) {
        let retired = {
            let mut state = self.state.lock();
            core::mem::replace(&mut *state, State::Closed)
        };
        // Master destruction may enter DRM; never run it under the tracking lock.
        drop(retired);
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
