// SPDX-License-Identifier: GPL-2.0-only

//! Renderer-local private-image names and retained completed content.

use super::{private_image::Image, render_job::Rendered};
use crate::image_storage::MAX_IMAGES;
use core::sync::atomic::{
    AtomicUsize,
    Ordering, //
};
use kernel::{prelude::*, sync::Arc};

struct Entry {
    id: u64,
    image: Arc<Image>,
    completed: Option<Arc<Rendered>>,
    pins: Arc<AtomicUsize>,
}

/// Registration names and storage retained for an exact output geometry.
///
/// This is not a source claim, a private-image write reservation, or completion of earlier
/// native work. The endpoint must independently establish authority and probe readiness before
/// publishing an offer. Drop outside endpoint and DRM locks; images can own native resources.
pub(crate) struct RegistrationSet {
    images: KVec<PinnedImage>,
}

struct PinnedImage {
    id: u64,
    image: Arc<Image>,
    pins: Arc<AtomicUsize>,
}

impl Drop for PinnedImage {
    fn drop(&mut self) {
        self.pins.fetch_sub(1, Ordering::Release);
    }
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl RegistrationSet {
    /// Exact registrations, not a promise of current write availability or valid pixels.
    pub(crate) fn images(&self) -> impl Iterator<Item = (u64, &Image)> {
        self.images.iter().map(|entry| (entry.id, &*entry.image))
    }
}

/// The owning endpoint serializes namespace operations, separately from native access.
/// Removed names never become reusable. Native work retains its own image registration
/// and memory charge independently of presence in this table.
pub(crate) struct Pool {
    entries: KVec<Entry>,
    last_id: u64,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Pool {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            entries: KVec::with_capacity(MAX_IMAGES, GFP_KERNEL)?,
            last_id: 0,
        })
    }

    pub(crate) fn check(&self, id: u64) -> Result {
        if id == 0 {
            return Err(EINVAL);
        }
        if self.last_id == u64::MAX {
            return Err(EOVERFLOW);
        }
        if id <= self.last_id {
            return Err(ESTALE);
        }
        if self.entries.len() == MAX_IMAGES {
            return Err(EBUSY);
        }
        Ok(())
    }

    /// Creation failure leaves the name available for retry. The creator checks authority.
    pub(crate) fn insert(
        &mut self,
        id: u64,
        create: impl FnOnce() -> Result<Arc<Image>>,
    ) -> Result {
        self.check(id)?;
        let pins = Arc::new(AtomicUsize::new(0), GFP_KERNEL)?;
        let image = create()?;
        self.entries
            .push_within_capacity(Entry {
                id,
                image,
                completed: None,
                pins,
            })
            .map_err(|_| EIO)?;
        self.last_id = id;
        Ok(())
    }

    pub(crate) fn image(&self, id: u64) -> Result<Arc<Image>> {
        Ok(self
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .ok_or(ENOENT)?
            .image
            .clone())
    }

    /// Pin existing names for one exact geometry without granting access to their contents.
    ///
    /// Namespace serialization excludes concurrent removal while the set is constructed.
    /// Failure rolls back every pin. Additional registrations do not extend an existing set.
    /// Destroying the endpoint may drop the table after revoking offers; retained sets keep
    /// storage alive, but cannot authorize operations through a closed endpoint.
    pub(super) fn pin_dimensions(
        &self,
        owner: &Arc<()>,
        dimensions: [u32; 2],
    ) -> Result<RegistrationSet> {
        if dimensions.contains(&0) {
            return Err(EINVAL);
        }
        let count = self
            .entries
            .iter()
            .filter(|entry| {
                entry.image.dimensions() == dimensions && entry.image.check_owner(owner).is_ok()
            })
            .count();
        if count == 0 {
            return Err(ENODATA);
        }
        let mut images = KVec::with_capacity(count, GFP_KERNEL)?;
        for entry in self.entries.iter().filter(|entry| {
            entry.image.dimensions() == dimensions && entry.image.check_owner(owner).is_ok()
        }) {
            entry
                .pins
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| EOVERFLOW)?;
            let pinned = PinnedImage {
                id: entry.id,
                image: entry.image.clone(),
                pins: entry.pins.clone(),
            };
            images.push_within_capacity(pinned).map_err(|_| EIO)?;
        }
        Ok(RegistrationSet { images })
    }

    /// Retention grants no right to read pixels; every output claim rechecks authority.
    pub(crate) fn completed(&self, id: u64) -> Result<Arc<Rendered>> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .ok_or(ENOENT)?
            .completed
            .clone()
            .ok_or(ENODATA)
    }

    /// Return the previous retention for destruction outside endpoint exclusion.
    /// The new record must name the exact registered allocation, not just its dimensions.
    pub(crate) fn publish(
        &mut self,
        id: u64,
        completed: Arc<Rendered>,
    ) -> Result<Option<Arc<Rendered>>> {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or(ENOENT)?;
        if !core::ptr::eq(&*entry.image, completed.image()) {
            return Err(EINVAL);
        }
        Ok(entry.completed.replace(completed))
    }

    /// Withdraw completed content before attempting another private write. Drop the returned
    /// owner before preparing storage, outside endpoint exclusion. Outstanding native users
    /// still make that preparation return EBUSY; withdrawal is not access retirement.
    pub(crate) fn withdraw(&mut self, id: u64) -> Result<Option<Arc<Rendered>>> {
        Ok(self
            .entries
            .iter_mut()
            .find(|entry| entry.id == id)
            .ok_or(ENOENT)?
            .completed
            .take())
    }

    /// Transfer all table-owned storage for destruction outside endpoint exclusion.
    /// Removal does not end native accesses or release their independent accounting.
    pub(crate) fn remove(&mut self, id: u64) -> Result<impl Sized> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.id == id)
            .ok_or(ENOENT)?;
        if self.entries[index].pins.load(Ordering::Acquire) != 0 {
            return Err(EBUSY);
        }
        self.entries.remove(index).map_err(|_| EIO)
    }
}
