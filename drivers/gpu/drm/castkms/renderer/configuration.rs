// SPDX-License-Identifier: GPL-2.0-only

//! Private configuration of an immutable renderer backend, independent of active scanout.

use super::{
    permission::Access,
    private_image::Image,
    private_pool::Pool,
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
/// Multiple configurations may coexist; only atomic state can select a published entry.
pub(crate) struct Configuration {
    access: Access,
    interval: crate::authority::Interval,
    owner: Arc<()>,
    profile: Profile,
    dimensions: [u32; 2],
}

impl Configuration {
    pub(crate) fn new(access: Access, profile: Profile, dimensions: [u32; 2]) -> Result<Self> {
        let profile = profile.with_exact_output(dimensions)?;
        let constraints = access.display().constraints.as_ref().ok_or(EOPNOTSUPP)?;
        let interval = access.current_interval()?;
        constraints.check_profile(&profile)?;
        Ok(Self {
            access,
            interval,
            owner: Arc::new((), GFP_KERNEL)?,
            profile,
            dimensions,
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

    /// Pin only this configuration's existing registrations after successful private preparation.
    /// Endpoint serialization protects the pool; publication must recheck live authority.
    pub(crate) fn prepare_worker(
        &self,
        pool: &Pool,
        completion: Option<&Fence>,
    ) -> Result<ready::Owner> {
        match completion.map_or(kernel::dma_fence::Status::Complete(Ok(())), Fence::status) {
            kernel::dma_fence::Status::Pending => return Err(EBUSY),
            kernel::dma_fence::Status::Complete(Err(_)) => return Err(EREMOTEIO),
            kernel::dma_fence::Status::Complete(Ok(())) => (),
        }
        let registrations = pool.pin_dimensions(&self.owner, self.dimensions)?;
        self.access.with_output_interval(self.interval, || Ok(()))?;
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
