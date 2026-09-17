// SPDX-License-Identifier: GPL-2.0-only

//! Allocation accounting shared by private rendering and recipient output storage.

use kernel::{
    dma_buf::DmaBuf,
    prelude::*,
    sync::{aref::ARef, Arc, Mutex},
};

pub(crate) const MAX_IMAGES: usize = 128;
pub(crate) const MAX_BYTES: usize = 512 * 1024 * 1024;
const MAX_BUFFERS: usize = 4;

/// Independently bounded storage stages sharing one alias ledger.
#[derive(Clone, Copy)]
pub(crate) enum Pool {
    Private,
    Recipient,
}

#[derive(Clone, Copy)]
struct Account {
    images: usize,
    bytes: usize,
}

struct Storage {
    pool: Pool,
    buffers: KVec<ARef<DmaBuf>>,
    dimensions: [u32; 2],
    bytes: usize,
}

struct RegistryState {
    closed: bool,
    accounts: [Account; 2],
    images: KVec<Arc<Storage>>,
}

/// Device-wide accounting includes removed registrations retained by native work.
///
/// Alias tracking stays present until the last use releases its registration owner.
/// Distinct reservations cannot prove distinct physical backing; the trusted importer
/// must additionally reject exporter-specific aliases and incompatible recipient scopes.
/// Registration accounts for storage; each importer checks its required access mode.
#[pin_data]
pub(crate) struct Registry {
    #[pin]
    state: Mutex<RegistryState>,
}

impl Registry {
    pub(crate) fn new() -> Result<Arc<Self>> {
        let images = KVec::with_capacity(2 * MAX_IMAGES, GFP_KERNEL)?;
        Arc::pin_init(
            pin_init!(Self {
                state <- kernel::new_mutex!(RegistryState {
                    closed: false,
                    accounts: [Account { images: 0, bytes: 0 }; 2],
                    images,
                }),
            }),
            GFP_KERNEL,
        )
    }

    /// Retain allocations without mapping, waiting or granting pixel access.
    /// Each distinct allocation appears once, even if a native image has several planes.
    pub(crate) fn register(
        self: &Arc<Self>,
        pool: Pool,
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
                pool,
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
            let account = &state.accounts[pool as usize];
            if account.images == MAX_IMAGES || bytes > MAX_BYTES - account.bytes {
                return Err(EBUSY);
            }
            state
                .images
                .push_within_capacity(storage.clone())
                .map_err(|_| EIO)?;
            let account = &mut state.accounts[pool as usize];
            account.images += 1;
            account.bytes += bytes;
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

pub(crate) struct Registration {
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
                let account = &mut state.accounts[image.pool as usize];
                account.images -= 1;
                account.bytes -= image.bytes;
                Some(image)
            })
        };
        drop(retired);
    }
}

impl Registration {
    pub(crate) fn dimensions(&self) -> [u32; 2] {
        self.storage.dimensions
    }
    pub(crate) fn buffers(&self) -> &[ARef<DmaBuf>] {
        &self.storage.buffers
    }
}
