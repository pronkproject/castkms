// SPDX-License-Identifier: GPL-2.0-only

//! One renderer endpoint's candidate ownership, independent of file transport.

use super::{
    candidate::Candidate,
    permission::Access, //
};
use crate::{execution::Description, scene::Configuration};
use kernel::{
    prelude::*,
    sync::{Arc, Mutex}, //
};

enum Slot {
    Idle,
    Publishing,
    Active { id: u64, candidate: Candidate },
}

struct State {
    closed: bool,
    next_id: u64,
    slot: Slot,
}

#[pin_data]
pub(super) struct Session {
    access: Access,
    #[pin]
    state: Mutex<State>,
}

impl Session {
    pub(super) fn new(access: Access) -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                access,
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
        let candidate = Candidate::begin(self.access.clone())?;
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
                Slot::Idle => return Err(ENOENT),
            }
        };
        candidate.cancel();
        drop(candidate);
        Ok(())
    }

    pub(super) fn close(&self) {
        let candidate = {
            let mut state = self.state.lock();
            state.closed = true;
            match core::mem::replace(&mut state.slot, Slot::Idle) {
                Slot::Active { candidate, .. } => Some(candidate),
                _ => None,
            }
        };
        if let Some(candidate) = candidate {
            candidate.cancel();
            drop(candidate);
        }
    }
}

#[must_use = "dropping an unpublished candidate cancels its reservation"]
pub(super) struct Pending<'a> {
    session: &'a Session,
    id: u64,
    candidate: Option<Candidate>,
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

    fn candidate(&self) -> Result<&Candidate> {
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
