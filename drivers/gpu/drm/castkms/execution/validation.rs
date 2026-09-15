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

/// Monotonic within one validation owner, not comparable across outputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Epoch(u64);

struct Gate {
    proposal: u64,
    target: Contract,
}

/// The owner must serialize this state with installation, publication and cancellation.
///
/// This object does not authenticate proposal numbers, observe authority or activate a
/// renderer. The owner must perform those checks under the same serialization before
/// preparing installation. In particular, two compatible scene snapshots do not prove
/// that an older incompatible commit can no longer publish.
pub(crate) struct Validation {
    active: Contract,
    epoch: Epoch,
    gate: Option<Gate>,
}

/// Exclusive, uninstalled change. Dropping it leaves the validation state unchanged.
///
/// Move it into the native installation continuation's success-only callback. Retain
/// the enclosing owner lock throughout preparation and native installation. No other
/// transaction or cancellation can inspect intermediate validation state.
#[must_use = "commit only after successful native state installation"]
pub(crate) struct Installation<'a> {
    validation: &'a mut Validation,
    gate: Gate,
    epoch: Epoch,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Validation {
    pub(crate) fn new(active: Contract) -> Self {
        Self {
            active,
            epoch: Epoch(1),
            gate: None,
        }
    }

    pub(crate) fn epoch(&self) -> Epoch {
        self.epoch
    }

    /// Ordinary animation is checked against every installed contract, not its serial.
    pub(crate) fn check(&self, scene: SceneView<'_>) -> Result {
        self.active.check(scene)?;
        if let Some(gate) = &self.gate {
            gate.target.check(scene)?;
        }
        Ok(())
    }

    /// Validate again after waits. TEST_ONLY may drop the result, never commit it.
    /// The proposal must already be authenticated to this output and authority interval.
    pub(crate) fn prepare(
        &mut self,
        expected: Epoch,
        proposal: u64,
        target: Contract,
        scene: SceneView<'_>,
    ) -> Result<Installation<'_>> {
        if expected != self.epoch {
            return Err(ESTALE);
        }
        if proposal == 0 {
            return Err(EINVAL);
        }
        if self.gate.is_some() {
            return Err(EBUSY);
        }
        self.active.check(scene)?;
        target.check(scene)?;
        // Reserve an epoch for lifting the gate too: cancellation must remain possible.
        self.epoch.0.checked_add(2).ok_or(EOVERFLOW)?;
        let epoch = Epoch(self.epoch.0 + 1);
        Ok(Installation {
            validation: self,
            gate: Gate { proposal, target },
            epoch,
        })
    }

}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Installation<'_> {
    /// Infallible once native installation succeeds; no allocation or resource release.
    pub(crate) fn commit(self) {
        self.validation.gate = Some(self.gate);
        self.validation.epoch = self.epoch;
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
