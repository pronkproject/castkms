// SPDX-License-Identifier: GPL-2.0-only

//! Registration-owned shutdown of the driver's independently synchronized state.

use super::{
    authority::Authority,
    output::Output,
    scene::Scene,
    Driver, //
};
use kernel::{
    drm::auth::MasterRef,
    prelude::*,
    sync::Arc, //
};

#[pin_data]
pub(super) struct State {
    #[pin]
    pub(super) authority: Authority<MasterRef<Driver>>,
    #[pin]
    pub(super) output: Output<Scene>,
}

impl State {
    fn new() -> impl PinInit<Self> {
        pin_init!(Self {
            authority <- Authority::new(),
            output <- Output::new(),
        })
    }

    pub(super) fn close(&self) {
        self.authority.close();
        self.output.close();
    }
}

/// Closes state on registration failure as well as normal module shutdown. State-held
/// framebuffer and master references must not keep their owning DRM device alive forever.
pub(super) struct Owner(Arc<State>);

impl Owner {
    pub(super) fn new() -> Result<Self> {
        Ok(Self(Arc::pin_init(State::new(), GFP_KERNEL)?))
    }

    pub(super) fn state(&self) -> Arc<State> {
        self.0.clone()
    }

    pub(super) fn close(&self) {
        self.0.close();
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.close();
    }
}
