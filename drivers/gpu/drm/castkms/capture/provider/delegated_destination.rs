// SPDX-License-Identifier: GPL-2.0-only

//! Recipient-owned storage bound to one delegated capture scope.

use super::delegated::Delegated;
use crate::{
    capture::reuse::Dependencies,
    display_control::Current,
    image_storage::{Pool, Registration},
};
use core::sync::atomic::{AtomicBool, Ordering};
use kernel::{
    dma_buf::DmaBuf,
    dma_fence::Fence,
    dma_resv::Usage,
    drm::fourcc,
    prelude::*,
    sync::{aref::ARef, poll::PollCondVar, Arc, Mutex},
};

/// Checked complete rows for the initial delegated output layout, not a source limit.
#[derive(Clone, Copy)]
#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
pub(crate) struct Layout {
    pub(crate) dimensions: [u32; 2],
    pub(crate) pitch: usize,
    pub(crate) offset: usize,
}

struct State {
    busy: bool,
}

/// Immutable recipient provenance, retained through pending native writes.
///
/// Importers must supply backing compatible with every prior recipient of that allocation.
/// Neither re-registration nor a new grant revokes an ordinary DMA-BUF. The registry rejects
/// known aliases but cannot discover arbitrary fd forwarding or exporter-specific aliases.
#[pin_data]
pub(crate) struct Image {
    scope: Delegated,
    storage: Registration,
    layout: Layout,
    #[pin]
    state: Mutex<State>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Delegated {
    /// Register destination storage without mapping it or authorizing a write.
    /// The trusted output stage must define every exposed pixel, padding byte and unused
    /// channel bit. Allocation regions outside the rows must not contain private pixels.
    pub(crate) fn register_destination(
        &self,
        buffer: &ARef<DmaBuf>,
        format: u32,
        modifier: u64,
        pitch: usize,
        offset: usize,
    ) -> Result<Arc<Image>> {
        if format != fourcc::XRGB8888 || modifier != fourcc::FORMAT_MOD_LINEAR {
            return Err(EOPNOTSUPP);
        }
        if !buffer.is_writable() {
            return Err(EACCES);
        }
        let dimensions = self.dimensions();
        let row = (dimensions[0] as usize).checked_mul(4).ok_or(EOVERFLOW)?;
        if pitch < row || pitch % 4 != 0 || offset % 4 != 0 {
            return Err(EINVAL);
        }
        let end = pitch
            .checked_mul(dimensions[1] as usize)
            .and_then(|span| offset.checked_add(span))
            .ok_or(EOVERFLOW)?;
        if end > buffer.size() {
            return Err(EINVAL);
        }
        let check = |current: &Current<'_>| {
            if current.uses_reservation(buffer.reservation())? {
                Err(EINVAL)
            } else {
                Ok(())
            }
        };
        self.with_current(check)?;
        let storage = self.storage_registry().register(
            Pool::Recipient,
            dimensions,
            core::slice::from_ref(buffer),
        )?;
        let image = Arc::pin_init(
            pin_init!(Image {
                scope: self.clone(),
                storage,
                layout: Layout { dimensions, pitch, offset },
                state <- kernel::new_mutex!(State { busy: false }),
            }),
            GFP_KERNEL,
        )?;
        self.with_current(check)?;
        Ok(image)
    }
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Image {
    pub(super) fn scope(&self) -> &Delegated {
        &self.scope
    }

    pub(crate) fn layout(&self) -> Layout {
        self.layout
    }

    pub(crate) fn buffer(&self) -> &DmaBuf {
        &self.storage.buffers()[0]
    }

    /// Reserve exclusive destination use. Request names belong to individual queues;
    /// the retained use object identifies storage ownership across queue incarnations.
    /// Demand stays unbound to compositor sources.
    /// Explicit reuse covers all prior external users; a reservation snapshot alone cannot
    /// exclude racing native submissions or recover previously discarded error records.
    /// External users must stop submitting new work before reserving the destination.
    pub(crate) fn reserve(self: &Arc<Self>, reuse: Option<ARef<Fence>>) -> Result<Arc<Use>> {
        self.reserve_notified(reuse, None)
    }

    pub(super) fn reserve_notified(
        self: &Arc<Self>,
        reuse: Option<ARef<Fence>>,
        changed: Option<Arc<PollCondVar>>,
    ) -> Result<Arc<Use>> {
        let dependencies = Dependencies::new(
            reuse.as_deref(),
            &self.buffer().reservation().snapshot(Usage::Read)?,
            changed,
        )?;
        let usage = Arc::pin_init(
            pin_init!(Use {
                image: self.clone(),
                dependencies <- kernel::new_mutex!(dependencies),
                active: AtomicBool::new(false),
            }),
            GFP_KERNEL,
        )?;
        self.scope.with_current(|current| {
            if current.uses_reservation(self.buffer().reservation())? {
                return Err(EINVAL);
            }
            let mut state = self.state.lock();
            if state.busy {
                return Err(EBUSY);
            }
            state.busy = true;
            usage.active.store(true, Ordering::Relaxed);
            Ok(())
        })?;
        Ok(usage)
    }
}

/// One exclusive provider use. Owning it grants no pixel access or native exclusion.
#[pin_data(PinnedDrop)]
pub(crate) struct Use {
    image: Arc<Image>,
    #[pin]
    dependencies: Mutex<Dependencies>,
    active: AtomicBool,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Use {
    pub(super) fn image(&self) -> &Image {
        &self.image
    }

    /// End provider exclusion after native access ends, before publishing a terminal result.
    /// Retained metadata references must not clear a newer reservation when later dropped.
    pub(super) fn retire(&self) {
        if self.active.swap(false, Ordering::Relaxed) {
            self.image.state.lock().busy = false;
        }
    }

    /// Typed readiness: a completed EAGAIN is an error, never a pending dependency.
    /// This observation is not native exclusion. The recipient and trusted renderer must
    /// serialize subsequent external use until the output stage has released its write.
    pub(crate) fn ready(&self) -> Result<bool> {
        let snapshot = self.image.buffer().reservation().snapshot(Usage::Read)?;
        let mut dependencies = self.dependencies.lock();
        dependencies.observe(&snapshot)?;
        dependencies.ready()
    }
}

#[pinned_drop]
impl PinnedDrop for Use {
    fn drop(self: Pin<&mut Self>) {
        self.retire();
    }
}
