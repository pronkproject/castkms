// SPDX-License-Identifier: GPL-2.0-only

//! Published sink description for the virtual connector.

use crate::{display, Driver};
use kernel::{
    drm::{
        device::{Registered, RegisteredDeviceRef},
        kms::connector::{self, Edid},
        Device,
    },
    prelude::*,
    sync::{Arc, Mutex},
};

enum Description {
    Attached(Option<Edid>),
    Disconnected,
}

enum State {
    Unmanaged,
    Managed {
        identity: Arc<()>,
        description: Description,
    },
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
                state <- kernel::new_mutex!(State::Unmanaged),
            }),
            GFP_KERNEL,
        )
    }

    pub(crate) fn status(&self) -> connector::Status {
        match &*self.state.lock() {
            State::Unmanaged
            | State::Managed {
                description: Description::Attached(_),
                ..
            } => connector::Status::Connected,
            State::Managed {
                description: Description::Disconnected,
                ..
            }
            | State::Closed => connector::Status::Disconnected,
        }
    }

    pub(crate) fn get_modes(
        &self,
        connector: &connector::ConnectorGuard<'_, display::Connector>,
    ) -> i32 {
        let state = self.state.lock();
        match &*state {
            State::Managed {
                description: Description::Attached(Some(edid)),
                ..
            } => match connector.add_edid_modes(edid) {
                Ok(count) if count > 0 => count,
                Ok(_) => Self::add_fallback_modes(connector),
                Err(_) => 0,
            },
            State::Unmanaged
            | State::Managed {
                description: Description::Attached(None),
                ..
            } => {
                if connector.update_edid(None).is_err() {
                    0
                } else {
                    Self::add_fallback_modes(connector)
                }
            }
            State::Managed {
                description: Description::Disconnected,
                ..
            }
            | State::Closed => {
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

    pub(crate) fn acquire(
        self: &Arc<Self>,
        device: &Device<Driver, Registered>,
    ) -> Result<Control> {
        if !Arc::ptr_eq(self, &device.monitor) {
            return Err(EINVAL);
        }
        let identity = Arc::new((), GFP_KERNEL)?;
        {
            let mut state = self.state.lock();
            match &*state {
                State::Unmanaged => {
                    *state = State::Managed {
                        identity: identity.clone(),
                        description: Description::Disconnected,
                    };
                }
                State::Managed { .. } => return Err(EBUSY),
                State::Closed => return Err(ENODEV),
            }
        }
        let control = Control {
            monitor: self.clone(),
            device: device.to_registered_ref(),
            identity,
        };
        control.notify();
        Ok(control)
    }

    fn publish(&self, identity: &Arc<()>, replacement: Description) -> Result {
        let retired = {
            let mut state = self.state.lock();
            match &*state {
                State::Managed {
                    identity: current, ..
                } if Arc::ptr_eq(current, identity) => core::mem::replace(
                    &mut *state,
                    State::Managed {
                        identity: identity.clone(),
                        description: replacement,
                    },
                ),
                State::Managed { .. } | State::Unmanaged => return Err(ECANCELED),
                State::Closed => return Err(ENODEV),
            }
        };
        drop(retired);
        Ok(())
    }

    fn release(&self, identity: &Arc<()>) -> bool {
        let retired = {
            let mut state = self.state.lock();
            match &*state {
                State::Managed {
                    identity: current, ..
                } if Arc::ptr_eq(current, identity) => {
                    Some(core::mem::replace(&mut *state, State::Unmanaged))
                }
                _ => None,
            }
        };
        let changed = retired.is_some();
        drop(retired);
        changed
    }

    pub(crate) fn close(&self) {
        let retired = core::mem::replace(&mut *self.state.lock(), State::Closed);
        drop(retired);
    }
}

/// Exclusive control of one virtual monitor publication interval.
pub(crate) struct Control {
    monitor: Arc<Monitor>,
    device: RegisteredDeviceRef<Driver>,
    identity: Arc<()>,
}

impl Control {
    pub(crate) fn attach(&self, edid: Option<Edid>) -> Result {
        self.monitor
            .publish(&self.identity, Description::Attached(edid))?;
        self.notify();
        Ok(())
    }

    pub(crate) fn detach(&self) -> Result {
        self.monitor
            .publish(&self.identity, Description::Disconnected)?;
        self.notify();
        Ok(())
    }

    fn notify(&self) {
        if let Some(device) = self.device.registration_guard() {
            device.hotplug_event();
        }
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        if self.monitor.release(&self.identity) {
            self.notify();
        }
    }
}
