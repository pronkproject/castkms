// SPDX-License-Identifier: GPL-2.0-only

//! Registration-owned shutdown of the driver's independently synchronized state.

use super::{
    authority::Authority,
    capture::{
        budget,
        grants,
        streams, //
    },
    host_compositor::configuration,
    Driver,
    Output, //
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
    pub(super) output: Arc<Output>,
    pub(super) host: Arc<configuration::Configuration>,
    pub(super) capture_grants: Arc<grants::Registry>,
    pub(super) capture_streams: Arc<streams::Registry>,
    pub(super) capture_budget: Arc<budget::Budget>,
}

impl State {
    fn new(
        output: Arc<Output>,
        host: Arc<configuration::Configuration>,
    ) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            authority <- Authority::new(),
            output,
            host,
            capture_grants: grants::Registry::new()?,
            capture_streams: streams::Registry::new()?,
            capture_budget: budget::Budget::new()?,
        })
    }

    fn close(&self) {
        self.capture_grants.close();
        self.capture_streams.close();
        self.authority.close();
        self.output.close();
    }
}

/// Closes state on registration failure as well as normal module shutdown. State-held
/// framebuffer and master references must not keep their owning DRM device alive forever.
pub(super) struct Owner {
    state: Arc<State>,
    host: configuration::Owner,
}

impl Owner {
    pub(super) fn new() -> Result<Self> {
        let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
        let host = configuration::Owner::new(output.clone())?;
        let state = Arc::pin_init(State::new(output, host.configuration()), GFP_KERNEL)?;
        Ok(Self { state, host })
    }

    pub(super) fn state(&self) -> Arc<State> {
        self.state.clone()
    }

    pub(super) fn close(&self) {
        self.state.close();
        self.host.close();
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.close();
    }
}
