// SPDX-License-Identifier: GPL-2.0-only

//! Borrowed complete-scene metadata, independent of descriptor transport.

use crate::scene::{Primary, Scene};
use kernel::prelude::*;

pub(crate) const MAX_LAYERS: usize = crate::scene::MAX_PLANES;
pub(crate) const MAX_COLOR_OPERATIONS: usize = 16;

/// Metadata borrows the claimed scene; it neither maps pixels nor extends read authority.
pub(crate) struct Description<'a> {
    pub(crate) layers: KVec<&'a Primary>,
    pub(crate) output: [u32; 2],
    pub(crate) content_serial: u64,
    pub(crate) color: Option<&'a crate::color::OutputColor>,
}

impl<'a> Description<'a> {
    pub(crate) fn new(scene: &'a Scene) -> Result<Self> {
        let mut layers = KVec::with_capacity(MAX_LAYERS, GFP_KERNEL)?;
        for layer in scene.layers() {
            if layers.len() == MAX_LAYERS
                || layer.framebuffer().plane_count() == 0
                || layer.framebuffer().plane_count() > 4
                || layer
                    .color
                    .as_ref()
                    .is_some_and(|color| color.operations().len() > MAX_COLOR_OPERATIONS)
            {
                return Err(E2BIG);
            }
            layers.push(layer, GFP_KERNEL)?;
        }
        let output = layers.first().ok_or(ENODATA)?.geometry().output;
        if layers.iter().any(|layer| layer.geometry().output != output) {
            return Err(EINVAL);
        }
        // Stable sorting preserves KMS plane creation order for equal zpos.
        // Insertion sort avoids another allocation for this bounded array.
        for index in 1..layers.len() {
            let mut current = index;
            while current > 0 && layers[current - 1].zpos > layers[current].zpos {
                layers.swap(current - 1, current);
                current -= 1;
            }
        }
        Ok(Self {
            layers,
            output,
            content_serial: scene.content_serial().ok_or(ENODATA)?.get(),
            color: scene.output_color.as_deref(),
        })
    }
}
