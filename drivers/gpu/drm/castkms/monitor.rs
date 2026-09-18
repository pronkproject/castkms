// SPDX-License-Identifier: GPL-2.0-only

//! Published sink description for the virtual connector.

pub(crate) mod group;

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
    Attached {
        edid: Option<Edid>,
        #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
        audio: Option<crate::audio::Attachment>,
    },
    Disconnected,
}

enum State {
    Unmanaged,
    Reserved { identity: Arc<()> },
    Managed {
        identity: Arc<()>,
        description: Description,
    },
    Closed,
}

#[pin_data]
pub(crate) struct Monitor {
    pub(crate) cec: Arc<crate::cec::Cec>,
    #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
    pub(crate) audio_link: Arc<crate::audio::playback::Gate>,
    #[pin]
    state: Mutex<State>,
}

impl Monitor {
    pub(crate) fn new() -> Result<Arc<Self>> {
        Arc::pin_init(
            try_pin_init!(Self {
                cec: crate::cec::Cec::new()?,
                #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
                audio_link: crate::audio::playback::Gate::new(false)?,
                state <- kernel::new_mutex!(State::Unmanaged),
            }),
            GFP_KERNEL,
        )
    }

    pub(crate) fn status(&self) -> connector::Status {
        match &*self.state.lock() {
            State::Managed {
                description: Description::Attached { .. },
                ..
            } => connector::Status::Connected,
            State::Unmanaged
            | State::Reserved { .. }
            | State::Managed {
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
        let count = match &*state {
            State::Managed {
                description:
                    Description::Attached {
                        edid: Some(edid), ..
                    },
                ..
            } => match connector.add_edid_modes(edid) {
                Ok(count) if count > 0 => count,
                Ok(_) => Self::add_fallback_modes(connector),
                Err(_) => 0,
            },
            State::Managed {
                description: Description::Attached { edid: None, .. },
                ..
            } => {
                if connector.update_edid(None).is_err() {
                    0
                } else {
                    Self::add_fallback_modes(connector)
                }
            }
            State::Unmanaged
            | State::Reserved { .. }
            | State::Managed {
                description: Description::Disconnected,
                ..
            }
            | State::Closed => {
                let _ = connector.update_edid(None);
                0
            }
        };
        drop(state);
        self.cec.refresh_physical_address();
        count
    }

    fn add_fallback_modes(connector: &connector::ConnectorGuard<'_, display::Connector>) -> i32 {
        let count = connector.add_modes_noedid((crate::execution::potential::MAX_DIMENSION,
            crate::execution::potential::MAX_DIMENSION));
        connector.set_preferred_mode((1920, 1080));
        count
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn acquire(
        self: &Arc<Self>,
        device: &Device<Driver, Registered>,
    ) -> Result<Control> {
        self.reserve(device)?.publish()
    }

    /// Reserve exclusive issuance without changing the disconnected connector.
    pub(crate) fn reserve(
        self: &Arc<Self>,
        device: &Device<Driver, Registered>,
    ) -> Result<PendingControl> {
        if !device
            .displays
            .iter()
            .any(|display| Arc::ptr_eq(self, &display.monitor))
        {
            return Err(EINVAL);
        }
        let identity = Arc::new((), GFP_KERNEL)?;
        {
            let mut state = self.state.lock();
            match &*state {
                State::Unmanaged => {
                    *state = State::Reserved {
                        identity: identity.clone(),
                    };
                }
                State::Reserved { .. } | State::Managed { .. } => return Err(EBUSY),
                State::Closed => return Err(ENODEV),
            }
        }
        let control = Control {
            monitor: self.clone(),
            device: device.to_registered_ref(),
            identity,
        };
        Ok(PendingControl { control })
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
                State::Managed { .. } | State::Reserved { .. } | State::Unmanaged => return Err(ECANCELED),
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
                State::Reserved { identity: current } if Arc::ptr_eq(current, identity) => {
                    *state = State::Unmanaged;
                    return false;
                }
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
        self.cec.close();
    }

    #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
    pub(crate) fn audio(&self) -> Result<Arc<crate::audio::Source>> {
        match &*self.state.lock() {
            State::Managed {
                description:
                    Description::Attached {
                        audio: Some(audio), ..
                    },
                ..
            } => Ok(audio.source.clone()),
            State::Closed => Err(ENODEV),
            _ => Err(ENOTCONN),
        }
    }
}

/// Exclusive control of one virtual monitor publication interval.
pub(crate) struct Control {
    monitor: Arc<Monitor>,
    device: RegisteredDeviceRef<Driver>,
    identity: Arc<()>,
}

/// Dropping an unpublished reservation preserves disconnected state without notification.
pub(crate) struct PendingControl {
    control: Control,
}

impl PendingControl {
    pub(crate) fn publish(self) -> Result<Control> {
        let control = self.control;
        {
            let mut state = control.monitor.state.lock();
            match &*state {
                State::Reserved { identity } if Arc::ptr_eq(identity, &control.identity) => {
                    *state = State::Managed {
                        identity: control.identity.clone(),
                        description: Description::Disconnected,
                    };
                }
                State::Closed => return Err(ENODEV),
                _ => return Err(ECANCELED),
            }
        }
        control.notify();
        Ok(control)
    }
}

impl Control {
    pub(crate) fn cec(&self) -> &Arc<crate::cec::Cec> {
        &self.monitor.cec
    }

    fn description(&self, edid: Option<Edid>) -> Result<Description> {
        #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
        let audio = {
            let device = self.device.registration_guard().ok_or(ENODEV)?;
            let index = device
                .displays
                .iter()
                .position(|display| Arc::ptr_eq(&display.monitor, &self.monitor))
                .ok_or(EINVAL)?;
            match &edid {
                Some(edid) => crate::audio::Attachment::new(
                    &device,
                    edid,
                    index,
                    self.monitor.audio_link.clone(),
                )?,
                None => None,
            }
        };
        Ok(Description::Attached {
            edid,
            #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
            audio,
        })
    }

    pub(crate) fn attach(&self, edid: Option<Edid>) -> Result {
        self.monitor.publish(
            &self.identity,
            self.description(edid)?,
        )?;
        self.monitor.cec.set_attached(true);
        self.notify();
        Ok(())
    }

    pub(crate) fn detach(&self) -> Result {
        self.monitor
            .publish(&self.identity, Description::Disconnected)?;
        self.monitor.cec.set_attached(false);
        self.notify();
        Ok(())
    }

    fn notify(&self) {
        if let Some(device) = self.device.registration_guard() {
            device.changed.notify_all();
            device.hotplug_event();
        }
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        if self.monitor.release(&self.identity) {
            self.monitor.cec.reset();
            self.notify();
        }
    }
}
