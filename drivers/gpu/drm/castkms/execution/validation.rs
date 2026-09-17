// SPDX-License-Identifier: GPL-2.0-only

//! Complete-scene views shared by native constraints validation.

use crate::scene::Scene;

/// Disabled is distinct from an enabled output with an empty scene.
#[derive(Clone, Copy)]
pub(crate) enum SceneView<'a> {
    Disabled,
    Enabled { scene: &'a Scene, output: [u32; 2] },
}

/// Check a complete scene against the built-in compositor's contract.
pub(crate) fn check_host(view: SceneView<'_>) -> kernel::error::Result {
    let SceneView::Enabled { scene, output } = view else {
        return Ok(());
    };
    if output.contains(&0)
        || output[0] > super::host::MAX_WIDTH
        || output[1] > super::host::MAX_HEIGHT
    {
        return Err(kernel::error::code::EOPNOTSUPP);
    }
    for layer in scene.layers() {
        if layer.geometry().output != output {
            return Err(kernel::error::code::EINVAL);
        }
        super::host::check_framebuffer(layer.framebuffer(), layer.geometry())?;
    }
    Ok(())
}
