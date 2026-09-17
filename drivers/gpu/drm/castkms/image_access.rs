// SPDX-License-Identifier: GPL-2.0-only

//! Current ownership checks for completed images, separate from their transport.

use crate::{
    display_control,
    host_compositor::{
        compose::Completed,
        layout::Layout, //
    },
    output::Identity,
    scene::Configuration,
    Driver, //
};
use kernel::{
    dma_buf::DmaBuf,
    drm::auth::MasterRef,
    prelude::*, //
};

/// Callback-local ownership of the displayed image, without storage or source access.
///
/// Construct from stabilized display control. Its lifetime and task restrictions follow
/// that control; successful checks do not authorize a later operation after the callback.
pub(crate) struct Current<'a> {
    control: display_control::Current<'a>,
    layout: Layout,
}

impl<'a> Current<'a> {
    /// Require current pixel attribution and HOST execution in addition to output control.
    pub(crate) fn new(control: display_control::Current<'a>) -> Result<Self> {
        control.check_scene_owner()?;
        control.check_host_image()?;
        let [width, height] = control.configuration().dimensions();
        Ok(Self {
            control,
            layout: Layout::new(width, height)?,
        })
    }

    pub(crate) fn configuration(&self) -> &Configuration {
        self.control.configuration()
    }

    pub(crate) fn layout(&self) -> Layout {
        self.layout
    }

    /// Reject storage known to overlap the currently displayed source.
    ///
    /// Shared reservations identify aliases even through distinct exports or imports.
    /// Different reservations do not prove distinct physical backing. This observation
    /// neither reserves the destination nor prevents a later KMS update from selecting it.
    pub(crate) fn check_destination(&self, buffer: &DmaBuf) -> Result {
        if self.control.uses_reservation(buffer.reservation())? {
            return Err(EINVAL);
        }
        Ok(())
    }

    /// Check independently completed pixels without confusing retention with permission.
    ///
    /// A completed image may precede the latest content update within the same display
    /// interval. Its original content serial remains unchanged; it is not relabeled current.
    pub(crate) fn check_image(&self, image: &Completed) -> Result {
        self.check_origin(
            image.output_identity(),
            image.configuration(),
            image.layout(),
            image.owner(),
        )
    }

    fn check_origin(
        &self,
        output: &Identity,
        configuration: Option<&Configuration>,
        layout: Layout,
        owner: Option<&MasterRef<Driver>>,
    ) -> Result {
        if output != self.control.output_identity()
            || configuration != Some(self.control.configuration())
            || layout != self.layout
            || owner != Some(self.control.master())
        {
            return Err(EACCES);
        }
        Ok(())
    }
}
