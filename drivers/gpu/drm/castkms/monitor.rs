// SPDX-License-Identifier: GPL-2.0-only

//! Published sink description for the virtual connector.

use crate::display;
use kernel::{
    drm::kms::connector::{self, Edid},
    prelude::*,
    sync::{Arc, Mutex},
};

pub(crate) enum Description {
    Fallback,
    Attached(Option<Edid>),
    Disconnected,
}

enum State {
    Published(Description),
    Closed,
}

#[pin_data]
pub(crate) struct Monitor {
    #[pin]
    state: Mutex<State>,
}

impl Monitor {
    pub(crate) fn new() -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                state <- kernel::new_mutex!(State::Published(Description::Fallback)),
            }),
            GFP_KERNEL,
        )
    }

    pub(crate) fn status(&self) -> connector::Status {
        match &*self.state.lock() {
            State::Published(Description::Fallback | Description::Attached(_)) => {
                connector::Status::Connected
            }
            State::Published(Description::Disconnected) | State::Closed => {
                connector::Status::Disconnected
            }
        }
    }

    pub(crate) fn get_modes(
        &self,
        connector: &connector::ConnectorGuard<'_, display::Connector>,
    ) -> i32 {
        let state = self.state.lock();
        match &*state {
            State::Published(Description::Attached(Some(edid))) => {
                match connector.add_edid_modes(edid) {
                    Ok(count) if count > 0 => count,
                    Ok(_) => Self::add_fallback_modes(connector),
                    Err(_) => 0,
                }
            }
            State::Published(Description::Fallback | Description::Attached(None)) => {
                if connector.update_edid(None).is_err() {
                    0
                } else {
                    Self::add_fallback_modes(connector)
                }
            }
            State::Published(Description::Disconnected) | State::Closed => {
                let _ = connector.update_edid(None);
                0
            }
        }
    }

    fn add_fallback_modes(connector: &connector::ConnectorGuard<'_, display::Connector>) -> i32 {
        let count = connector.add_modes_noedid((1920, 1080));
        connector.set_preferred_mode((1920, 1080));
        count
    }

    #[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
    pub(crate) fn publish(&self, replacement: Description) -> Result {
        let retired = {
            let mut state = self.state.lock();
            if matches!(*state, State::Closed) {
                return Err(ENODEV);
            }
            core::mem::replace(&mut *state, State::Published(replacement))
        };
        drop(retired);
        Ok(())
    }

    pub(crate) fn close(&self) {
        let retired = core::mem::replace(&mut *self.state.lock(), State::Closed);
        drop(retired);
    }
}
