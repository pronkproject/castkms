// SPDX-License-Identifier: GPL-2.0-only

//! Registration-owned shutdown of the driver's independently synchronized state.

use super::{
    authority::Authority,
    capture::{
        budget,
        grants,
        streams, //
    },
    execution::publication::Publication,
    host_compositor::configuration,
    monitor::Monitor,
    renderer_startup,
    Driver,
    Output, //
};
use kernel::{
    alloc::kvec::KVec,
    drm::auth::MasterRef,
    prelude::*,
    sync::Arc, //
};

pub(super) const MAX_OUTPUTS: u32 = 8;

/// State whose lifetime and synchronization are private to one display pipeline.
pub(super) struct Display {
    pub(super) execution: Arc<Publication>,
    pub(super) output: Arc<Output>,
    pub(super) monitor: Arc<Monitor>,
    pub(super) host: Arc<configuration::Configuration>,
    pub(super) startup: Arc<renderer_startup::Startup>,
}

#[pin_data]
pub(super) struct State {
    pub(super) enable_cursor: bool,
    pub(super) enable_overlay: bool,
    pub(super) enable_plane_pipeline: bool,
    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) execution: Arc<Publication>,
    #[pin]
    pub(super) authority: Authority<MasterRef<Driver>>,
    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) output: Arc<Output>,
    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) monitor: Arc<Monitor>,
    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) host: Arc<configuration::Configuration>,
    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) startup: Arc<renderer_startup::Startup>,
    pub(super) capture_grants: Arc<grants::Registry>,
    pub(super) capture_streams: Arc<streams::Registry>,
    pub(super) capture_budget: Arc<budget::Budget>,
    pub(super) displays: KVec<Arc<Display>>,
}

impl State {
    fn new(
        displays: KVec<Arc<Display>>,
        enable_cursor: bool,
        enable_overlay: bool,
        enable_plane_pipeline: bool,
    ) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            enable_cursor,
            enable_overlay,
            enable_plane_pipeline,
            #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
            execution: displays.first().ok_or(EINVAL)?.execution.clone(),
            authority <- Authority::new(),
            #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
            output: displays.first().ok_or(EINVAL)?.output.clone(),
            #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
            monitor: displays.first().ok_or(EINVAL)?.monitor.clone(),
            #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
            host: displays.first().ok_or(EINVAL)?.host.clone(),
            #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
            startup: displays.first().ok_or(EINVAL)?.startup.clone(),
            capture_grants: grants::Registry::new()?,
            capture_streams: streams::Registry::new()?,
            capture_budget: budget::Budget::new()?,
            displays,
        })
    }

    fn close(&self) {
        for display in &self.displays {
            display.monitor.close();
        }
        self.capture_grants.close();
        self.capture_streams.close();
        self.authority.close();
        for display in &self.displays {
            display.output.close();
        }
        for display in &self.displays {
            display.execution.close();
        }
    }
}

/// Closes state on registration failure as well as normal module shutdown. State-held
/// framebuffer and master references must not keep their owning DRM device alive forever.
pub(super) struct Owner {
    state: Arc<State>,
    displays: KVec<DisplayOwner>,
}

struct DisplayOwner {
    host: configuration::Owner,
    startup: renderer_startup::Owner,
}

impl Owner {
    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) fn new() -> Result<Self> {
        Self::new_outputs(1)
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) fn new_outputs(count: u32) -> Result<Self> {
        Self::new_features(count, false, false, false)
    }

    pub(super) fn new_features(count: u32, enable_cursor: bool, enable_overlay: bool, enable_plane_pipeline: bool) -> Result<Self> {
        if count == 0 || count > MAX_OUTPUTS {
            return Err(EINVAL);
        }
        let mut owners = KVec::with_capacity(count as usize, GFP_KERNEL)?;
        let mut displays = KVec::with_capacity(count as usize, GFP_KERNEL)?;
        for _ in 0..count {
            let execution = Arc::pin_init(Publication::new(), GFP_KERNEL)?;
            let output = Arc::pin_init(Output::new(), GFP_KERNEL)?;
            let monitor = Monitor::new()?;
            let host = configuration::Owner::new(output.clone(), execution.clone())?;
            let startup = renderer_startup::Owner::new(output.identity())?;
            displays.push(
                Arc::new(
                    Display {
                        execution,
                        output,
                        monitor,
                        host: host.configuration(),
                        startup: startup.startup(),
                    },
                    GFP_KERNEL,
                )?,
                GFP_KERNEL,
            )?;
            owners.push(DisplayOwner { host, startup }, GFP_KERNEL)?;
        }
        let state = Arc::pin_init(
            State::new(displays, enable_cursor, enable_overlay, enable_plane_pipeline),
            GFP_KERNEL,
        )?;
        Ok(Self {
            state,
            displays: owners,
        })
    }

    pub(super) fn state(&self) -> Arc<State> {
        self.state.clone()
    }

    pub(super) fn close(&self) {
        for display in &self.displays {
            display.startup.close();
        }
        self.state.close();
        for display in &self.displays {
            display.host.close();
        }
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.close();
    }
}
