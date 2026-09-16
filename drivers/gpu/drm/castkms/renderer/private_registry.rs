// SPDX-License-Identifier: GPL-2.0-only

//! Private allocation accounting retained across renderer replacement and native cleanup.

use kernel::{
    dma_buf::DmaBuf,
    prelude::*,
    sync::{aref::ARef, Arc, Mutex},
};

const MAX_IMAGES: usize = 128;
const MAX_BYTES: usize = 512 * 1024 * 1024;
const MAX_BUFFERS: usize = 4;

struct Storage {
    buffers: KVec<ARef<DmaBuf>>,
    dimensions: [u32; 2],
    bytes: usize,
}

struct RegistryState {
    closed: bool,
    bytes: usize,
    images: KVec<Arc<Storage>>,
}

/// Device-wide accounting includes removed registrations retained by native work.
///
/// Alias tracking stays present until the last use releases its registration owner.
/// Distinct reservations cannot prove distinct physical backing; the trusted importer
/// must additionally reject exporter-specific aliases and sharing with recipients.
#[pin_data]
pub(crate) struct Registry {
    #[pin]
    state: Mutex<RegistryState>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Registry {
    pub(crate) fn new() -> Result<Arc<Self>> {
        let images = KVec::with_capacity(MAX_IMAGES, GFP_KERNEL)?;
        Arc::pin_init(
            pin_init!(Self {
                state <- kernel::new_mutex!(RegistryState { closed: false, bytes: 0, images }),
            }),
            GFP_KERNEL,
        )
    }

    /// Retain private allocations without mapping, waiting or granting pixel access.
    /// Each distinct allocation appears once, even if a native image has several planes.
    pub(super) fn register(
        self: &Arc<Self>,
        dimensions: [u32; 2],
        buffers: &[ARef<DmaBuf>],
    ) -> Result<Registration> {
        if dimensions
            .iter()
            .any(|&n| n == 0 || n > crate::execution::potential::MAX_DIMENSION)
            || buffers.is_empty()
            || buffers.len() > MAX_BUFFERS
        {
            return Err(EINVAL);
        }
        let mut bytes = 0usize;
        let mut retained = KVec::with_capacity(buffers.len(), GFP_KERNEL)?;
        for (index, buffer) in buffers.iter().enumerate() {
            if !buffer.is_readable() || !buffer.is_writable() {
                return Err(EACCES);
            }
            if buffer.size() == 0
                || buffers[..index]
                    .iter()
                    .any(|previous| aliases(previous, buffer))
            {
                return Err(EINVAL);
            }
            bytes = bytes.checked_add(buffer.size()).ok_or(EOVERFLOW)?;
            retained.push(buffer.clone(), GFP_KERNEL)?;
        }
        if bytes > MAX_BYTES {
            return Err(E2BIG);
        }
        let storage = Arc::new(
            Storage {
                buffers: retained,
                dimensions,
                bytes,
            },
            GFP_KERNEL,
        )?;
        {
            let mut state = self.state.lock();
            if state.closed {
                return Err(ENODEV);
            }
            if state.images.iter().any(|image| {
                image
                    .buffers
                    .iter()
                    .any(|a| buffers.iter().any(|b| aliases(a, b)))
            }) {
                return Err(EEXIST);
            }
            if state.images.len() == MAX_IMAGES || bytes > MAX_BYTES - state.bytes {
                return Err(EBUSY);
            }
            state
                .images
                .push_within_capacity(storage.clone())
                .map_err(|_| EIO)?;
            state.bytes += bytes;
        }
        Ok(Registration {
            registry: self.clone(),
            storage,
        })
    }

    pub(crate) fn close(&self) {
        self.state.lock().closed = true;
    }
}

fn aliases(a: &DmaBuf, b: &DmaBuf) -> bool {
    core::ptr::eq(a, b) || core::ptr::eq(a.reservation(), b.reservation())
}

pub(super) struct Registration {
    registry: Arc<Registry>,
    storage: Arc<Storage>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let retired = {
            let mut state = self.registry.state.lock();
            let index = state
                .images
                .iter()
                .position(|image| Arc::ptr_eq(image, &self.storage));
            index.and_then(|index| {
                let image = state.images.remove(index).ok()?;
                state.bytes -= image.bytes;
                Some(image)
            })
        };
        drop(retired);
    }
}

impl Registration {
    pub(super) fn dimensions(&self) -> [u32; 2] {
        self.storage.dimensions
    }
    pub(super) fn buffers(&self) -> &[ARef<DmaBuf>] {
        &self.storage.buffers
    }
}
