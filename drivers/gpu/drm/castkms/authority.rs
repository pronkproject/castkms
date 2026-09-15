// SPDX-License-Identifier: GPL-2.0-only

//! Top-level master observations, separate from scene ownership and lease authorization.

pub(crate) mod grants;

use kernel::{
    prelude::*,
    sync::Mutex, //
};

enum State<I> {
    Tracking { master: Option<I>, interval: u64 },
    Closed,
}

/// One uninterrupted control interval within its originating authority tracker.
///
/// Equality is meaningful only for observations of the same tracker. Retaining an
/// interval does not stabilize control or authorize an operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Interval(u64);

#[pin_data]
pub(super) struct Authority<I> {
    #[pin]
    state: Mutex<State<I>>,
}

impl<I: Unpin> Authority<I> {
    pub(super) fn new() -> impl PinInit<Self> {
        pin_init!(Self {
            state <- kernel::new_mutex!(State::Tracking { master: None, interval: 0 }),
        })
    }

    /// Called in native master transition order. Closing permanently rejects later events.
    pub(super) fn changed(&self, mut master: Option<I>) {
        let retired = {
            let mut state = self.state.lock();
            let next = match &*state {
                State::Tracking { interval, .. } => interval.checked_add(1),
                State::Closed => None,
            };
            // Closing also handles exhaustion; neither identities nor intervals wrap.
            let replacement = match next {
                Some(interval) => State::Tracking {
                    master: master.take(),
                    interval,
                },
                None => State::Closed,
            };
            core::mem::replace(&mut *state, replacement)
        };
        drop(retired);
        drop(master);
    }

    /// A coherent historical observation, never a permission to read pixels.
    pub(super) fn snapshot(&self) -> Option<I>
    where
        I: Clone,
    {
        match &*self.state.lock() {
            State::Tracking { master, .. } => master.clone(),
            State::Closed => None,
        }
    }

    /// Observe an interval while the caller separately stabilizes native master control.
    #[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
    pub(crate) fn interval(&self) -> Result<Interval> {
        match &*self.state.lock() {
            State::Tracking {
                master: Some(_),
                interval,
            } => Ok(Interval(*interval)),
            State::Tracking { master: None, .. } => Err(EACCES),
            State::Closed => Err(ENODEV),
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
