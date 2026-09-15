// SPDX-License-Identifier: GPL-2.0-only

//! Scene acceptance during a capability transition, without renderer authority.

use super::capabilities;
use crate::scene::Scene;
use kernel::{prelude::*, sync::Arc};

/// Disabled is distinct from an enabled output with an empty scene.
#[derive(Clone, Copy)]
pub(crate) enum SceneView<'a> {
    Disabled,
    Enabled { scene: &'a Scene, output: [u32; 2] },
}

/// An immutable input contract, independent of execution or resource generations.
#[derive(Clone)]
pub(crate) enum Contract {
    Host,
    Renderer(Arc<capabilities::Profile>),
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Contract {
    /// Check metadata only. Success grants neither source access nor GPU import support.
    pub(crate) fn check(&self, view: SceneView<'_>) -> Result {
        let SceneView::Enabled { scene, output } = view else {
            return Ok(());
        };
        match self {
            Self::Renderer(profile) => profile.check(scene, output),
            Self::Host => {
                if output.contains(&0)
                    || output[0] > super::host::MAX_WIDTH
                    || output[1] > super::host::MAX_HEIGHT
                {
                    return Err(EOPNOTSUPP);
                }
                for layer in scene.layers() {
                    if layer.geometry().output != output {
                        return Err(EINVAL);
                    }
                    super::host::check_framebuffer(layer.framebuffer(), layer.geometry())?;
                }
                // HOST implements every operation in Scene's color types. LUT sizes
                // are bounded at construction; checking never evaluates pixels.
                Ok(())
            }
        }
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
