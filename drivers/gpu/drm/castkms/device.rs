// SPDX-License-Identifier: GPL-2.0-only

//! Registration-owned shutdown of the driver's independently synchronized state.

use super::{
    authority::{
        grants,
        Authority, //
    },
    capture::{
        budget,
        streams, //
    },
    execution::publication::Publication,
    host_compositor::configuration,
    monitor::Monitor,
    Driver,
    Output, //
};
use kernel::{
    alloc::kvec::KVec,
    drm::auth::MasterRef,
    prelude::*,
    sync::{poll::PollCondVar, Arc, SetOnce}, //
};

pub(super) const MAX_OUTPUTS: u32 = 8;

/// State whose lifetime and synchronization are private to one display pipeline.
pub(super) struct Display {
    pub(super) execution: Arc<Publication>,
    pub(super) output: Arc<Output>,
    pub(super) monitor: Arc<Monitor>,
    pub(super) host: Arc<configuration::Configuration>,
    pub(crate) constraints: SetOnce<Arc<crate::execution::constraints::provider::Provider>>,
}

#[pin_data]
pub(super) struct State {
    pub(crate) constraints_enabled: bool,
    /// Advisory wakeups only. Consumers register before rechecking their exact authority.
    pub(crate) changed: Arc<PollCondVar>,
    pub(crate) validation: Arc<crate::execution::coordinator::Coordinator>,
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
    pub(super) capture_grants: Arc<grants::Registry>,
    pub(super) renderer_workers: Arc<grants::Registry>,
    #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
    pub(super) audio_grants: Arc<crate::audio::provider::Registry>,
    pub(super) capture_streams: Arc<streams::Registry>,
    pub(super) capture_budget: Arc<budget::Budget>,
    pub(crate) capture_request_budget: Arc<crate::capture::request_budget::Budget>,
    pub(crate) image_storage: Arc<crate::image_storage::Registry>,
    pub(crate) monitor_groups: Arc<crate::monitor::group::Registry>,
    pub(super) displays: KVec<Arc<Display>>,
}

impl State {
    fn new(
        displays: KVec<Arc<Display>>,
        changed: Arc<PollCondVar>,
        enable_cursor: bool,
        enable_overlay: bool,
        enable_plane_pipeline: bool,
        constraints_enabled: bool,
    ) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            constraints_enabled,
            changed,
            validation: Arc::pin_init(
                crate::execution::coordinator::Coordinator::new(displays.len()),
                GFP_KERNEL,
            )?,
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
            capture_grants: grants::Registry::new()?,
            renderer_workers: grants::Registry::new_device_workers()?,
            #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
            audio_grants: crate::audio::provider::Registry::new()?,
            capture_streams: streams::Registry::new()?,
            capture_budget: budget::Budget::new()?,
            capture_request_budget: crate::capture::request_budget::Budget::new()?,
            image_storage: crate::image_storage::Registry::new()?,
            monitor_groups: crate::monitor::group::Registry::new()?,
            displays,
        })
    }

    fn close(&self) {
        for display in &self.displays {
            if let Some(provider) = display.constraints.as_ref() {
                provider.close();
            }
        }
        self.image_storage.close();
        self.monitor_groups.close();
        self.validation.close();
        for display in &self.displays {
            display.monitor.close();
        }
        self.capture_grants.close();
        self.renderer_workers.close();
        #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
        self.audio_grants.close();
        self.capture_streams.close();
        self.authority.close();
        for display in &self.displays {
            display.output.close();
        }
        for display in &self.displays {
            display.execution.close();
        }
        self.changed.notify_all();
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
}

impl Owner {
    pub(super) fn new_features(count: u32, enable_cursor: bool, enable_overlay: bool, enable_plane_pipeline: bool) -> Result<Self> {
        Self::new_configuration(count, enable_cursor, enable_overlay, enable_plane_pipeline, true)
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) fn new_constraints(count: u32) -> Result<Self> {
        Self::new_configuration(count, true, true, true, true)
    }

    pub(super) fn new_configuration(count: u32, enable_cursor: bool, enable_overlay: bool,
        enable_plane_pipeline: bool, constraints_enabled: bool) -> Result<Self> {
        if count == 0 || count > MAX_OUTPUTS {
            return Err(EINVAL);
        }
        let mut owners = KVec::with_capacity(count as usize, GFP_KERNEL)?;
        let mut displays = KVec::with_capacity(count as usize, GFP_KERNEL)?;
        let changed = Arc::pin_init(kernel::new_poll_condvar!(), GFP_KERNEL)?;
        for _ in 0..count {
            let execution = Arc::pin_init(Publication::new(), GFP_KERNEL)?;
            let output = Arc::pin_init(Output::new_notified(Some(changed.clone())), GFP_KERNEL)?;
            let monitor = Monitor::new()?;
            let host = configuration::Owner::new(output.clone(), execution.clone())?;
            displays.push(
                Arc::new(
                    Display {
                        execution,
                        output,
                        monitor,
                        host: host.configuration(),
                        constraints: SetOnce::new(),
                    },
                    GFP_KERNEL,
                )?,
                GFP_KERNEL,
            )?;
            owners.push(DisplayOwner { host }, GFP_KERNEL)?;
        }
        let state = Arc::pin_init(
            State::new(displays, changed, enable_cursor, enable_overlay, enable_plane_pipeline,
                constraints_enabled),
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
