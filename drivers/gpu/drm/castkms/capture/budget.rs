// SPDX-License-Identifier: GPL-2.0-only

//! Bounded private capture storage, independent of source reads and media queues.

use crate::host_compositor::layout::Layout;
use kernel::{
    prelude::*,
    sync::{
        Arc,
        Mutex, //
    }, //
};

const BYTE_LIMIT: usize = 64 * 1024 * 1024;
const STREAM_LIMIT: usize = 16;
pub(super) const CAPACITY_LIMIT: u32 = 8;

struct Used {
    bytes: usize,
    streams: usize,
}

/// A device's private capture allocation limits, including retained results from old streams.
#[pin_data]
pub(crate) struct Budget {
    #[pin]
    used: Mutex<Used>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Budget {
    pub(crate) fn new() -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                used <- kernel::new_mutex!(Used { bytes: 0, streams: 0 }),
            }),
            GFP_KERNEL,
        )
    }

    /// Reserve the whole queue before creating a stream, without waiting for old storage.
    ///
    /// These are limits for private kernel results, not display cadence or transport windows.
    /// Capacity also bounds per-request bookkeeping for small images. Keep the reservation
    /// until every native result and active job associated with the stream has been released.
    pub(crate) fn reserve(self: &Arc<Self>, layout: Layout, capacity: u32) -> Result<Charge> {
        if capacity == 0 {
            return Err(EINVAL);
        }
        if capacity > CAPACITY_LIMIT {
            return Err(E2BIG);
        }
        let bytes = layout
            .size()
            .checked_mul(capacity as usize)
            .ok_or(EOVERFLOW)?;
        if bytes > BYTE_LIMIT {
            return Err(E2BIG);
        }
        let mut used = self.used.lock();
        if used.streams == STREAM_LIMIT || bytes > BYTE_LIMIT - used.bytes {
            return Err(EBUSY);
        }
        used.bytes += bytes;
        used.streams += 1;
        Ok(Charge {
            budget: self.clone(),
            bytes,
            layout,
            capacity,
        })
    }
}

/// Unique reservation for one stream's maximum storage; not tied to a public handle's close.
pub(crate) struct Charge {
    budget: Arc<Budget>,
    bytes: usize,
    layout: Layout,
    capacity: u32,
}

impl Charge {
    pub(super) fn layout(&self) -> Layout {
        self.layout
    }

    pub(super) fn capacity(&self) -> u32 {
        self.capacity
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        let mut used = self.budget.used.lock();
        used.bytes -= self.bytes;
        used.streams -= 1;
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
