// SPDX-License-Identifier: GPL-2.0-only

//! Client-local output registrations, separate from private result storage and source depth.

use super::resources::Resources;
use crate::capture::destination::Image;
use kernel::{
    prelude::*,
    sync::Arc, //
};

const REGISTRATION_LIMIT: usize = 16;
const ALLOCATION_LIMIT: usize = 16 * 1024 * 1024;

pub(super) struct Destinations {
    images: Resources<Arc<Image>>,
}

impl Destinations {
    pub(super) fn new() -> Result<Self> {
        Ok(Self {
            images: Resources::new(REGISTRATION_LIMIT)?,
        })
    }

    pub(super) fn insert(&mut self, id: u64, image: Image) -> Result {
        self.images.insert(id, || {
            if !image.buffer().is_writable() {
                return Err(EACCES);
            }
            // Limit retained allocations, not only the visible rows within each one.
            // No pages are mapped or pinned by registration itself.
            if image.buffer().size() > ALLOCATION_LIMIT {
                return Err(E2BIG);
            }
            Ok(Arc::new(image, GFP_KERNEL)?)
        })
    }

    pub(super) fn get(&self, id: u64) -> Result<Arc<Image>> {
        Ok(self.images.get(id)?.clone())
    }

    pub(super) fn remove(&mut self, id: u64) -> Result {
        drop(self.images.remove(id)?);
        Ok(())
    }
}
