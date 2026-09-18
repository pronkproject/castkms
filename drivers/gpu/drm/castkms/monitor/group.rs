// SPDX-License-Identifier: GPL-2.0-only

//! Transactional publication and lifetime control for monitor groups.

use super::{Control as MemberControl, Description, Monitor, State};
use crate::{device::MAX_OUTPUTS, Driver};
use kernel::{
    alloc::kvec::KVec,
    drm::{device::Registered, kms::connector::Edid, Device},
    prelude::*,
    sync::{Arc, MutexGuard},
};

/// An unpublished, all-member reservation for one monitor group.
pub(crate) struct Pending {
    controls: KVec<MemberControl>,
}

/// Exclusive control of one virtual monitor group publication interval.
pub(crate) struct Control {
    controls: KVec<MemberControl>,
}

impl Monitor {
    /// Reserve several monitors in display order without publishing any member.
    pub(crate) fn reserve_group(
        device: &Device<Driver, Registered>,
        members: &[usize],
    ) -> Result<Pending> {
        if members.is_empty() || members.len() > MAX_OUTPUTS as usize {
            return Err(EINVAL);
        }
        let mut controls = KVec::with_capacity(members.len(), GFP_KERNEL)?;
        let mut previous = None;
        for &member in members {
            if previous.is_some_and(|previous| member <= previous) {
                return Err(EINVAL);
            }
            let display = device.displays.get(member).ok_or(EINVAL)?;
            let pending = display.monitor.reserve(device)?;
            controls.push(pending.control, GFP_KERNEL)?;
            previous = Some(member);
        }
        Ok(Pending { controls })
    }
}

impl Pending {
    pub(crate) fn publish(self) -> Result<Control> {
        let controls = self.controls;
        let mut states: [Option<MutexGuard<'_, State>>; MAX_OUTPUTS as usize] =
            core::array::from_fn(|_| None);
        for (slot, control) in states.iter_mut().zip(&controls) {
            *slot = Some(control.monitor.state.lock());
        }
        for (state, control) in states.iter().flatten().zip(&controls) {
            match &**state {
                State::Reserved { identity } if Arc::ptr_eq(identity, &control.identity) => {}
                State::Closed => return Err(ENODEV),
                _ => return Err(ECANCELED),
            }
        }
        for (state, control) in states.iter_mut().flatten().zip(&controls) {
            **state = State::Managed {
                identity: control.identity.clone(),
                description: Description::Disconnected,
            };
        }
        drop(states);
        controls.first().ok_or(EINVAL)?.notify();
        Ok(Control { controls })
    }
}

impl Control {
    fn descriptions(&self, edids: KVec<Option<Edid>>) -> Result<KVec<Description>> {
        if edids.len() != self.controls.len() {
            return Err(EINVAL);
        }
        let mut descriptions = KVec::with_capacity(edids.len(), GFP_KERNEL)?;
        for (control, edid) in self.controls.iter().zip(edids) {
            descriptions.push(control.description(edid)?, GFP_KERNEL)?;
        }
        Ok(descriptions)
    }

    fn publish(&self, descriptions: KVec<Description>) -> Result {
        let mut states: [Option<MutexGuard<'_, State>>; MAX_OUTPUTS as usize] =
            core::array::from_fn(|_| None);
        let mut retired: [Option<State>; MAX_OUTPUTS as usize] = core::array::from_fn(|_| None);
        for (slot, control) in states.iter_mut().zip(&self.controls) {
            *slot = Some(control.monitor.state.lock());
        }
        for (state, control) in states.iter().flatten().zip(&self.controls) {
            match &**state {
                State::Managed { identity, .. } if Arc::ptr_eq(identity, &control.identity) => {}
                State::Closed => return Err(ENODEV),
                _ => return Err(ECANCELED),
            }
        }
        for (index, ((state, control), description)) in states
            .iter_mut()
            .flatten()
            .zip(&self.controls)
            .zip(descriptions)
            .enumerate()
        {
            retired[index] = Some(core::mem::replace(
                &mut **state,
                State::Managed {
                    identity: control.identity.clone(),
                    description,
                },
            ));
        }
        drop(states);
        drop(retired);
        Ok(())
    }

    pub(crate) fn attach(&self, edids: KVec<Option<Edid>>) -> Result {
        self.publish(self.descriptions(edids)?)?;
        for control in &self.controls {
            control.monitor.cec.set_attached(true);
        }
        self.controls.first().ok_or(EINVAL)?.notify();
        Ok(())
    }

    pub(crate) fn detach(&self) -> Result {
        let mut descriptions = KVec::with_capacity(self.controls.len(), GFP_KERNEL)?;
        for _ in &self.controls {
            descriptions.push(Description::Disconnected, GFP_KERNEL)?;
        }
        self.publish(descriptions)?;
        for control in &self.controls {
            control.monitor.cec.set_attached(false);
        }
        self.controls.first().ok_or(EINVAL)?.notify();
        Ok(())
    }

    fn release(&self) -> bool {
        let mut states: [Option<MutexGuard<'_, State>>; MAX_OUTPUTS as usize] =
            core::array::from_fn(|_| None);
        let mut retired: [Option<State>; MAX_OUTPUTS as usize] = core::array::from_fn(|_| None);
        for (slot, control) in states.iter_mut().zip(&self.controls) {
            *slot = Some(control.monitor.state.lock());
        }
        let mut changed = false;
        for (index, (state, control)) in states.iter_mut().flatten().zip(&self.controls).enumerate()
        {
            let matches = matches!(
                &**state,
                State::Managed { identity, .. } if Arc::ptr_eq(identity, &control.identity)
            );
            if matches {
                retired[index] = Some(core::mem::replace(&mut **state, State::Unmanaged));
                changed = true;
            }
        }
        drop(states);
        drop(retired);
        changed
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        if self.release() {
            for control in &self.controls {
                control.monitor.cec.reset();
            }
            if let Some(control) = self.controls.first() {
                control.notify();
            }
        }
    }
}
