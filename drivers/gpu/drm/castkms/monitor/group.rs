// SPDX-License-Identifier: GPL-2.0-only

//! Transactional publication and lifetime control for monitor groups.

use super::{Control as MemberControl, Description, Monitor, State};
use crate::{device::MAX_OUTPUTS, Driver};
use kernel::{
    alloc::kvec::KVec,
    drm::{
        device::Registered,
        kms::connector::{Edid, EdidTile},
        Device,
    },
    prelude::*,
    sync::{Arc, MutexGuard},
};

/// Complete rectangular topology decoded from a monitor group's EDIDs.
pub(crate) struct Topology {
    identity: [u8; 9],
    horizontal_tiles: u8,
    vertical_tiles: u8,
    tile_width: u16,
    tile_height: u16,
    member_locations: [[u8; 2]; MAX_OUTPUTS as usize],
    member_count: usize,
}

impl Topology {
    pub(crate) fn from_edids(edids: &[Edid]) -> Result<Self> {
        let first = edids.first().ok_or(EINVAL)?.tile()?.ok_or(EINVAL)?;
        let first_tile = first.tile();
        let member_count = usize::from(first_tile.horizontal_tiles())
            .checked_mul(usize::from(first_tile.vertical_tiles()))
            .ok_or(EOVERFLOW)?;
        if member_count != edids.len() || member_count > MAX_OUTPUTS as usize {
            return Err(EINVAL);
        }
        let aggregate_width = u32::from(first_tile.width())
            .checked_mul(u32::from(first_tile.horizontal_tiles()))
            .ok_or(EOVERFLOW)?;
        let aggregate_height = u32::from(first_tile.height())
            .checked_mul(u32::from(first_tile.vertical_tiles()))
            .ok_or(EOVERFLOW)?;
        if aggregate_width > crate::execution::potential::MAX_DIMENSION
            || aggregate_height > crate::execution::potential::MAX_DIMENSION
            || !first_tile.is_single_monitor()
        {
            return Err(EINVAL);
        }

        let mut member_locations = [[0; 2]; MAX_OUTPUTS as usize];
        let mut occupied = 0u8;
        for (index, edid) in edids.iter().enumerate() {
            let member = edid.tile()?.ok_or(EINVAL)?;
            Self::validate_member(&first, &member)?;
            let tile = member.tile();
            let position = tile
                .vertical_location()
                .checked_mul(tile.horizontal_tiles())
                .and_then(|row| row.checked_add(tile.horizontal_location()))
                .ok_or(EOVERFLOW)?;
            let bit = 1u8.checked_shl(u32::from(position)).ok_or(EOVERFLOW)?;
            if occupied & bit != 0 {
                return Err(EINVAL);
            }
            occupied |= bit;
            member_locations[index] = [tile.horizontal_location(), tile.vertical_location()];
        }
        let complete = u8::MAX >> (u8::BITS as usize - member_count);
        if occupied != complete {
            return Err(EINVAL);
        }

        Ok(Self {
            identity: *first.topology_id(),
            horizontal_tiles: first_tile.horizontal_tiles(),
            vertical_tiles: first_tile.vertical_tiles(),
            tile_width: first_tile.width(),
            tile_height: first_tile.height(),
            member_locations,
            member_count,
        })
    }

    fn validate_member(first: &EdidTile, member: &EdidTile) -> Result {
        let expected = first.tile();
        let tile = member.tile();
        if member.topology_id() != first.topology_id()
            || tile.horizontal_tiles() != expected.horizontal_tiles()
            || tile.vertical_tiles() != expected.vertical_tiles()
            || tile.width() != expected.width()
            || tile.height() != expected.height()
            || !tile.is_single_monitor()
        {
            return Err(EINVAL);
        }
        Ok(())
    }

    pub(crate) fn identity(&self) -> &[u8; 9] {
        &self.identity
    }

    pub(crate) fn dimensions(&self) -> [u8; 2] {
        [self.horizontal_tiles, self.vertical_tiles]
    }

    pub(crate) fn tile_size(&self) -> [u16; 2] {
        [self.tile_width, self.tile_height]
    }

    pub(crate) fn member_location(&self, index: usize) -> Option<[u8; 2]> {
        (index < self.member_count).then(|| self.member_locations[index])
    }
}

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
        for (index, (slot, control)) in states.iter_mut().zip(&controls).enumerate() {
            *slot = Some(control.monitor.state.lock_nested(index as u32));
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
    fn descriptions(&self, edids: KVec<Edid>) -> Result<KVec<Description>> {
        if edids.len() != self.controls.len() {
            return Err(EINVAL);
        }
        let mut descriptions = KVec::with_capacity(edids.len(), GFP_KERNEL)?;
        for edid in edids {
            descriptions.push(
                Description::Attached {
                    edid: Some(edid),
                    #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
                    audio: None,
                },
                GFP_KERNEL,
            )?;
        }
        Ok(descriptions)
    }

    fn publish(&self, descriptions: KVec<Description>) -> Result {
        let mut states: [Option<MutexGuard<'_, State>>; MAX_OUTPUTS as usize] =
            core::array::from_fn(|_| None);
        let mut retired: [Option<State>; MAX_OUTPUTS as usize] = core::array::from_fn(|_| None);
        for (index, (slot, control)) in states.iter_mut().zip(&self.controls).enumerate() {
            *slot = Some(control.monitor.state.lock_nested(index as u32));
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

    pub(crate) fn attach(&self, edids: KVec<Edid>) -> Result<Topology> {
        let topology = Topology::from_edids(&edids)?;
        self.publish(self.descriptions(edids)?)?;
        self.controls.first().ok_or(EINVAL)?.notify();
        Ok(topology)
    }

    pub(crate) fn detach(&self) -> Result {
        let mut descriptions = KVec::with_capacity(self.controls.len(), GFP_KERNEL)?;
        for _ in &self.controls {
            descriptions.push(Description::Disconnected, GFP_KERNEL)?;
        }
        self.publish(descriptions)?;
        self.controls.first().ok_or(EINVAL)?.notify();
        Ok(())
    }

    fn release(&self) -> bool {
        let mut states: [Option<MutexGuard<'_, State>>; MAX_OUTPUTS as usize] =
            core::array::from_fn(|_| None);
        let mut retired: [Option<State>; MAX_OUTPUTS as usize] = core::array::from_fn(|_| None);
        for (index, (slot, control)) in states.iter_mut().zip(&self.controls).enumerate() {
            *slot = Some(control.monitor.state.lock_nested(index as u32));
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
            if let Some(control) = self.controls.first() {
                control.notify();
            }
        }
    }
}
