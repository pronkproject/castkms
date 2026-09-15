// SPDX-License-Identifier: GPL-2.0-only

//! One renderer endpoint's candidate and active ownership, independent of file transport.

use super::{
    candidate::Candidate,
    permission::Access,
    probe::Source as ProbeSource, //
};
use crate::{
    execution::Description,
    renderer_startup,
    scene::Configuration,
    Driver, //
};
use kernel::{
    drm::device::RegisteredDeviceRef,
    prelude::*,
    sync::{Arc, Mutex}, //
};

enum Slot {
    Idle,
    Publishing,
    Active {
        id: u64,
        candidate: Arc<Candidate>,
    },
    Activating {
        id: u64,
        candidate: Arc<Candidate>,
    },
    Renderer {
        id: u64,
        candidate: Arc<Candidate>,
        active: renderer_startup::Active,
        _source: ProbeSource,
        description: Description,
    },
}

struct State {
    closed: bool,
    next_id: u64,
    slot: Slot,
}

#[pin_data]
pub(super) struct Session {
    access: Access,
    device: RegisteredDeviceRef<Driver>,
    #[pin]
    state: Mutex<State>,
}

impl Session {
    pub(super) fn new(access: Access, device: RegisteredDeviceRef<Driver>) -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                access,
                device,
                state <- kernel::new_mutex!(State {
                    closed: false,
                    next_id: 1,
                    slot: Slot::Idle,
                }),
            }),
            GFP_KERNEL,
        )
    }

    pub(super) fn description(&self) -> Result<Description> {
        self.access
            .with_current(|_| Ok(self.access.device().execution.describe()))
    }

    pub(super) fn begin(&self, expected_generation: u64) -> Result<Pending<'_>> {
        {
            let state = self.state.lock();
            if state.closed {
                return Err(EKEYREVOKED);
            }
            if !matches!(state.slot, Slot::Idle) {
                return Err(EBUSY);
            }
        }
        let candidate = Arc::new(Candidate::begin(self.access.clone())?, GFP_KERNEL)?;
        let id = {
            let mut state = self.state.lock();
            if state.closed {
                return Err(EKEYREVOKED);
            }
            if !matches!(state.slot, Slot::Idle) {
                return Err(EBUSY);
            }
            let id = state.next_id;
            state.next_id = id.checked_add(1).ok_or(EOVERFLOW)?;
            state.slot = Slot::Publishing;
            id
        };
        let pending = Pending {
            session: self,
            id,
            candidate: Some(candidate),
        };
        if pending.execution()?.generation != expected_generation {
            return Err(ESTALE);
        }
        pending.candidate()?.validate()?;
        Ok(pending)
    }

    pub(super) fn abort(&self, id: u64) -> Result {
        let candidate = {
            let mut state = self.state.lock();
            match core::mem::replace(&mut state.slot, Slot::Idle) {
                Slot::Active {
                    id: current,
                    candidate,
                } if current == id => candidate,
                other @ Slot::Active { .. } => {
                    state.slot = other;
                    return Err(ENOENT);
                }
                Slot::Publishing => {
                    state.slot = Slot::Publishing;
                    return Err(EBUSY);
                }
                other @ Slot::Activating { .. } => {
                    state.slot = other;
                    return Err(EBUSY);
                }
                other @ Slot::Renderer { id: current, .. } => {
                    state.slot = other;
                    return if current == id {
                        Err(EALREADY)
                    } else {
                        Err(ENOENT)
                    };
                }
                Slot::Idle => return Err(ENOENT),
            }
        };
        candidate.cancel();
        drop(candidate);
        Ok(())
    }

    /// Retain the named candidate for a pixel operation without holding the session lock.
    pub(super) fn candidate(&self, id: u64) -> Result<Arc<Candidate>> {
        let state = self.state.lock();
        if state.closed {
            return Err(EKEYREVOKED);
        }
        match &state.slot {
            Slot::Active {
                id: current,
                candidate,
            } if *current == id => Ok(candidate.clone()),
            Slot::Publishing | Slot::Activating { .. } => Err(EBUSY),
            Slot::Renderer { id: current, .. } if *current == id => Err(EALREADY),
            _ => Err(ENOENT),
        }
    }

    /// Activate one completed candidate or reconcile an already published result.
    pub(super) fn activate(&self, id: u64) -> Result<Description> {
        let registered = self.device.registration_guard().ok_or(ENODEV)?;
        let candidate = {
            let mut state = self.state.lock();
            if state.closed {
                return Err(EKEYREVOKED);
            }
            if let Slot::Renderer {
                id: current,
                active,
                description,
                ..
            } = &state.slot
            {
                return if *current == id {
                    active.check()?;
                    if self.access.device().execution.describe() == *description {
                        Ok(*description)
                    } else {
                        Err(EIO)
                    }
                } else {
                    Err(ENOENT)
                };
            }
            match core::mem::replace(&mut state.slot, Slot::Idle) {
                Slot::Active {
                    id: current,
                    candidate,
                } if current == id => {
                    state.slot = Slot::Activating {
                        id,
                        candidate: candidate.clone(),
                    };
                    candidate
                }
                other @ Slot::Publishing | other @ Slot::Activating { .. } => {
                    state.slot = other;
                    return Err(EBUSY);
                }
                other => {
                    state.slot = other;
                    return Err(ENOENT);
                }
            }
        };

        let activated = candidate.activate(&registered);
        let mut state = self.state.lock();
        if state.closed {
            state.slot = Slot::Idle;
            drop(state);
            drop(activated);
            return Err(EKEYREVOKED);
        }
        if !matches!(state.slot, Slot::Activating { id: current, .. } if current == id) {
            drop(state);
            drop(activated);
            return Err(ECANCELED);
        }
        match activated {
            Ok((active, source, description)) => {
                state.slot = Slot::Renderer {
                    id,
                    candidate,
                    active,
                    _source: source,
                    description,
                };
                drop(state);
                registered.hotplug_event();
                Ok(description)
            }
            Err(error) => {
                state.slot = Slot::Active { id, candidate };
                Err(error)
            }
        }
    }

    pub(super) fn close(&self) {
        let (candidate, active) = {
            let mut state = self.state.lock();
            state.closed = true;
            match core::mem::replace(&mut state.slot, Slot::Idle) {
                Slot::Active { candidate, .. } | Slot::Activating { candidate, .. } => {
                    (Some(candidate), None)
                }
                Slot::Renderer {
                    candidate, active, ..
                } => (Some(candidate), Some(active)),
                _ => (None, None),
            }
        };
        if let Some(candidate) = candidate {
            candidate.cancel();
            drop(candidate);
        }
        drop(active);
    }
}

#[must_use = "dropping an unpublished candidate cancels its reservation"]
pub(super) struct Pending<'a> {
    session: &'a Session,
    id: u64,
    candidate: Option<Arc<Candidate>>,
}

impl Pending<'_> {
    pub(super) fn id(&self) -> u64 {
        self.id
    }

    pub(super) fn configuration(&self) -> Result<&Configuration> {
        Ok(self.candidate()?.configuration())
    }

    pub(super) fn execution(&self) -> Result<Description> {
        Ok(self.candidate()?.execution())
    }

    fn candidate(&self) -> Result<&Arc<Candidate>> {
        self.candidate.as_ref().ok_or(EINVAL)
    }

    pub(super) fn publish(mut self) -> Result {
        self.candidate()?.validate()?;
        let candidate = self.candidate.take().ok_or(EINVAL)?;
        let mut state = self.session.state.lock();
        if state.closed {
            drop(state);
            candidate.cancel();
            drop(candidate);
            return Err(EKEYREVOKED);
        }
        if !matches!(state.slot, Slot::Publishing) {
            drop(state);
            candidate.cancel();
            drop(candidate);
            return Err(ECANCELED);
        }
        state.slot = Slot::Active {
            id: self.id,
            candidate,
        };
        Ok(())
    }
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        if let Some(candidate) = self.candidate.take() {
            let mut state = self.session.state.lock();
            if matches!(state.slot, Slot::Publishing) {
                state.slot = Slot::Idle;
            }
            drop(state);
            candidate.cancel();
            drop(candidate);
        }
    }
}
