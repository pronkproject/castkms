// SPDX-License-Identifier: GPL-2.0-only

//! Bounded renderer-private storage, independent of source and recipient lifetimes.
//!
//! The trusted renderer chooses and validates its native image layout. Registration
//! retains its backing allocations, not a CPU format interpretation. The allocations
//! must remain private to the rendering service, including every alias and import.

use crate::image_storage::{Registration, Registry};
use core::sync::atomic::{AtomicBool, Ordering};
use kernel::{
    dma_buf::DmaBuf,
    dma_fence::{
        retirement::{Retire, Retirement},
        Fence,
    },
    prelude::*,
    sync::{aref::ARef, Arc, Mutex},
};

struct ImageState {
    busy: bool,
    quarantined: bool,
    last_use: u64,
}

/// A registration for one renderer incarnation, not permission to write the allocation.
#[pin_data]
pub(crate) struct Image {
    registration: Registration,
    owner: Arc<()>,
    #[pin]
    state: Mutex<ImageState>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Image {
    pub(super) fn new(
        registry: &Arc<Registry>,
        owner: &Arc<()>,
        dimensions: [u32; 2],
        buffers: &[ARef<DmaBuf>],
    ) -> Result<Arc<Self>> {
        if buffers
            .iter()
            .any(|buffer| !buffer.is_readable() || !buffer.is_writable())
        {
            return Err(EACCES);
        }
        let registration = registry.register(dimensions, buffers)?;
        Arc::pin_init(
            pin_init!(Self {
                registration,
                owner: owner.clone(),
                state <- kernel::new_mutex!(ImageState { busy: false, quarantined: false, last_use: 0 }),
            }),
            GFP_KERNEL,
        )
    }

    pub(crate) fn dimensions(&self) -> [u32; 2] {
        self.registration.dimensions()
    }

    pub(crate) fn buffers(&self) -> &[ARef<DmaBuf>] {
        self.registration.buffers()
    }

    pub(super) fn check_owner(&self, owner: &Arc<()>) -> Result {
        if Arc::ptr_eq(&self.owner, owner) {
            Ok(())
        } else {
            Err(EACCES)
        }
    }

    /// Reserve independent storage before claiming any compositor source.
    /// Successful reservation consumes its use ID, even if later source admission fails.
    /// It authorizes no access; dropping it before claim releases the reservation.
    pub(crate) fn prepare(self: &Arc<Self>, use_id: u64) -> Result<Prepared> {
        if use_id == 0 {
            return Err(EINVAL);
        }
        let usage = Arc::new(
            Use {
                image: self.clone(),
                active: AtomicBool::new(false),
            },
            GFP_KERNEL,
        )?;
        let retirement = Retirement::new(Hold(usage.clone()))?;
        {
            let mut state = self.state.lock();
            if state.quarantined {
                return Err(EIO);
            }
            if state.last_use == u64::MAX {
                return Err(EOVERFLOW);
            }
            if use_id <= state.last_use {
                return Err(ESTALE);
            }
            if state.busy {
                return Err(EBUSY);
            }
            state.busy = true;
            state.last_use = use_id;
            usage.active.store(true, Ordering::Relaxed);
        }
        Ok(Prepared { usage, retirement })
    }
}

pub(super) struct Use {
    image: Arc<Image>,
    active: AtomicBool,
}

impl Use {
    pub(super) fn image(&self) -> &Image {
        &self.image
    }
}

impl Drop for Use {
    fn drop(&mut self) {
        if self.active.load(Ordering::Relaxed) {
            self.image.state.lock().busy = false;
        }
    }
}

struct Hold(Arc<Use>);

// SAFETY: CastKMS's module retains the payload and its storage cleanup implementation.
#[vtable]
unsafe impl Retire for Hold {
    fn retire(self) {
        drop(self.0);
    }
}

/// Storage and cleanup capacity reserved without binding a compositor source.
pub(crate) struct Prepared {
    usage: Arc<Use>,
    retirement: Retirement<Hold>,
}

impl Prepared {
    pub(super) fn image(&self) -> &Image {
        self.usage.image()
    }

    /// The caller admits the bounded stage under its live authority before conversion.
    pub(super) fn claim(self) -> Access {
        Access {
            usage: self.usage,
            retirement: Some(self.retirement),
        }
    }
}

/// One claimed native stage; unresolved abandonment quarantines its bounded storage.
///
/// Dropping an unresolved stage is not proof that access ended. Its cleanup owner and
/// module remain retained rather than making the allocation available to a later use.
pub(super) struct Access {
    usage: Arc<Use>,
    retirement: Option<Retirement<Hold>>,
}

impl Access {
    pub(super) fn image(&self) -> &Image {
        self.usage.image()
    }

    pub(super) fn finish(mut self, completion: Option<&Fence>) -> Arc<Use> {
        if let Some(retirement) = self.retirement.take() {
            match completion {
                Some(fence) => retirement.submit(fence),
                None => drop(retirement),
            }
        }
        self.usage.clone()
    }
}

impl Drop for Access {
    fn drop(&mut self) {
        if let Some(retirement) = self.retirement.take() {
            self.usage.image.state.lock().quarantined = true;
            core::mem::forget(retirement);
        }
    }
}
