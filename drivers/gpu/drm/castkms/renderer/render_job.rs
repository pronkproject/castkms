// SPDX-License-Identifier: GPL-2.0-only

//! An authorized source read bound to independently reserved private storage.

use super::{
    content::Released,
    job::{Completion, SourceJob},
    private_image::{Access, Image, Prepared, Use},
};
use kernel::sync::Arc;

/// One bounded source-to-private-image stage, not an output write to a recipient.
///
/// The renderer must isolate its native source queue and mappings from downstream waits.
/// Reserving storage here does not prove that an external GPU submission obeys that rule.
#[must_use = "a render job must report how all source and private-image access ended"]
pub(crate) struct RenderJob {
    source: SourceJob,
    destination: Access,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl RenderJob {
    pub(super) fn new(source: SourceJob, destination: Prepared) -> Self {
        Self {
            source,
            destination: destination.claim(),
        }
    }

    pub(crate) fn source(&self) -> &SourceJob {
        &self.source
    }

    pub(crate) fn destination(&self) -> &Image {
        self.destination.image()
    }

    /// End the complete stage. Submitted completion must cover source reads and private
    /// writes. No-access promises neither occurred; CPU completion includes coherency.
    pub(crate) fn release(self, completion: Completion) -> Option<Rendered> {
        let fence = match &completion {
            Completion::Submitted(fence) => Some(&**fence),
            Completion::Cpu | Completion::WithoutAccess => None,
        };
        let usage = self.destination.finish(fence);
        self.source
            .release(completion)
            .map(|content| Rendered { usage, content })
    }
}

/// Private storage held with the original source-stage evidence, not a capture grant.
///
/// Storage remains unavailable for overwrite while this record or a native stage retains
/// its use. Dropping it does not end a still-pending native write. No source claim survives.
pub(crate) struct Rendered {
    usage: Arc<Use>,
    content: Released,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Rendered {
    pub(crate) fn image(&self) -> &Image {
        self.usage.image()
    }

    pub(crate) fn content(&self) -> &Released {
        &self.content
    }
}
