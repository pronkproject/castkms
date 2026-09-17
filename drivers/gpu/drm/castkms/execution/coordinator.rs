// SPDX-License-Identifier: GPL-2.0-only

//! Device-wide exclusion for complete-cohort scene acceptance.

mod installation;
pub(crate) use installation::Update;

use super::validation::SceneView;
use kernel::{prelude::*, sync::{Mutex, MutexGuard}};

struct State {
    count: usize,
    closed: bool,
}

#[pin_data]
pub(crate) struct Coordinator {
    #[pin]
    state: Mutex<State>,
}

pub(crate) struct Guard<'a>(MutexGuard<'a, State>);

impl Coordinator {
    pub(crate) fn new(count: usize) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            state <- kernel::new_mutex!(State {
                count: if count == 0 || count > crate::device::MAX_OUTPUTS as usize {
                    return Err(EINVAL);
                } else { count },
                closed: false,
            }),
        })
    }

    pub(crate) fn lock(&self) -> Guard<'_> { Guard(self.state.lock()) }

    pub(crate) fn close(&self) {
        self.state.lock().closed = true;
    }
}

impl Guard<'_> {
    pub(crate) fn check(&self, output: usize, scene: SceneView<'_>) -> Result {
        if output >= self.0.count { return Err(EINVAL); }
        if self.0.closed {
            return match scene {
                SceneView::Disabled => Ok(()),
                SceneView::Enabled { .. } => Err(ENODEV),
            };
        }
        super::validation::check_host(scene)
    }
}
