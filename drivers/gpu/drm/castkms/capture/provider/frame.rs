// SPDX-License-Identifier: GPL-2.0-only

//! Captured pixels and their original description, without retained compositor storage.

use super::Request;
use crate::{
    host_compositor::{
        compose::Completed,
        layout::Layout, //
    },
    output::Identity,
    scene::{
        Configuration,
        ContentSerial, //
    }, //
};
use kernel::{
    prelude::*,
    time::{
        Instant,
        Monotonic, //
    }, //
};

/// Historical metadata, not the current scene or permission to acquire more pixels.
///
/// No framebuffer, source claim, private compositor slot or master reference is retained.
/// A blank image has no framebuffer content serial. The completion time describes private
/// CPU composition, not presentation, downstream delivery or a later copy of the image.
#[derive(Clone)]
pub(crate) struct Metadata {
    output: Identity,
    configuration: Configuration,
    layout: Layout,
    content: Option<ContentSerial>,
    completed_at: Instant<Monotonic>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Metadata {
    pub(crate) fn output_identity(&self) -> &Identity {
        &self.output
    }

    pub(crate) fn configuration(&self) -> &Configuration {
        &self.configuration
    }

    pub(crate) fn layout(&self) -> Layout {
        self.layout
    }

    pub(crate) fn content_serial(&self) -> Option<ContentSerial> {
        self.content
    }

    pub(crate) fn completed_at(&self) -> Instant<Monotonic> {
        self.completed_at
    }
}

/// One completed delivery with its own image description.
///
/// The request's terminal status still decides pixel validity; retaining metadata does not
/// turn a revoked or failed result into success. Stream close may discard the native result.
#[must_use = "dropping the frame releases its retained capture result"]
pub(crate) struct Frame {
    request: Request,
    metadata: Metadata,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Frame {
    pub(super) fn new(request: Request, image: &Completed) -> Result<Self> {
        Ok(Self {
            request,
            metadata: Metadata {
                output: image.output_identity().clone(),
                configuration: image.configuration().cloned().ok_or(EINVAL)?,
                layout: image.layout(),
                content: image.content_serial(),
                completed_at: image.completed_at(),
            },
        })
    }

    pub(crate) fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    pub(crate) fn request(&self) -> &Request {
        &self.request
    }

    /// Keep only the result for a caller that deliberately does not need image metadata.
    pub(crate) fn into_request(self) -> Request {
        self.request
    }
}
