// SPDX-License-Identifier: GPL-2.0-only

//! Private preparation of an immutable renderer offer, independent of active scanout.

use super::{
    permission::Access,
    private_image::Image,
    private_pool::Pool,
    probe::Probe,
    ready,
};
use crate::execution::capabilities::Profile;
use kernel::{
    dma_buf::DmaBuf,
    dma_fence::Fence,
    prelude::*,
    sync::{aref::ARef, Arc},
};

/// One endpoint's immutable declaration and private storage namespace.
/// Preparation does not reserve the output, require enabled video, or admit source reads.
/// Multiple drafts may coexist; only ordinary atomic state can select a published entry.
pub(crate) struct Draft {
    access: Access,
    interval: crate::authority::Interval,
    owner: Arc<()>,
    profile: Profile,
    dimensions: [u32; 2],
    probe: Arc<Probe>,
}

impl Draft {
    pub(crate) fn new(access: Access, profile: Profile, dimensions: [u32; 2]) -> Result<Self> {
        profile.check_output(dimensions)?;
        access.display().constraints.as_ref().ok_or(EOPNOTSUPP)?;
        let interval = access.current_interval()?;
        Ok(Self {
            access,
            interval,
            owner: Arc::new((), GFP_KERNEL)?,
            profile,
            dimensions,
            probe: Arc::pin_init(Probe::new(), GFP_KERNEL)?,
        })
    }

    pub(crate) fn access(&self) -> &Access {
        &self.access
    }

    pub(crate) fn dimensions(&self) -> [u32; 2] {
        self.dimensions
    }

    pub(crate) fn interval(&self) -> crate::authority::Interval {
        self.interval
    }

    /// Allocate outside native locks, rechecking authority and source independence afterward.
    /// The trusted renderer supplies the private layout; no CPU format interpretation applies.
    pub(crate) fn register_image(&self, buffers: &[ARef<DmaBuf>]) -> Result<Arc<Image>> {
        self.access.check_private_storage(self.interval, buffers)?;
        let image = Image::new(
            &self.access.device().image_storage,
            &self.owner,
            self.access.device().changed.clone(),
            self.dimensions,
            buffers,
        )?;
        self.access.check_private_storage(self.interval, buffers)?;
        Ok(image)
    }

    /// Report private compatibility work without borrowing display pixels or selecting this draft.
    pub(crate) fn submit_probe(&self, completion: Option<ARef<Fence>>) -> Result {
        self.access.with_output_interval(self.interval, || Ok(()))?;
        self.probe.submit_then(completion, || {
            self.access.with_output_interval(self.interval, || Ok(()))
        })
    }

    pub(crate) fn probe_status(&self) -> Result<kernel::dma_fence::Status> {
        self.access.with_output_interval(self.interval, || self.probe.status())
    }

    /// Pin only this draft's existing registrations after successful native probe completion.
    /// Endpoint serialization protects the pool; publication must recheck live authority.
    pub(crate) fn prepare_worker(&self, pool: &Pool) -> Result<ready::Owner> {
        let registrations = pool.pin_dimensions(&self.owner, self.dimensions)?;
        self.access.with_output_interval(self.interval, || self.probe.completed())?;
        let mut owner = ready::Owner::new(
            self.access.display().output.identity().clone(),
            self.interval,
            &self.profile,
            self.dimensions,
            registrations,
        )?;
        owner.track_permission(&self.access)?;
        Ok(owner)
    }
}
