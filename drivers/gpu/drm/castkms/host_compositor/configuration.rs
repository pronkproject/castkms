// SPDX-License-Identifier: GPL-2.0-only

//! Lazy worker replacement with registration-owned shutdown.

mod change;

use super::{
    budget::Budget,
    layout::Layout,
    pool::Pool,
    worker, //
};
use crate::{
    Driver,
    Output, //
};
use kernel::{
    drm::Device,
    prelude::*,
    sync::{
        Arc,
        Mutex, //
    }, //
};

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;

struct Active {
    layout: Layout,
    worker: worker::Owner,
}

enum State {
    Open(Option<Active>),
    Disabled,
    Closed,
}

/// One output's lazily allocated worker, without capture or source-read authority.
///
/// Configuration and shutdown take `lifecycle` before `state`. Worker callbacks take
/// neither lock. Mapping, buffer destruction and draining occur outside `state`, so
/// obtaining an existing handle does not wait for allocation or a running pixel copy.
#[pin_data]
pub(crate) struct Configuration {
    #[pin]
    lifecycle: Mutex<()>,
    #[pin]
    state: Mutex<State>,
    output: Arc<Output>,
    budget: Arc<Budget>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Configuration {
    /// Allocate on demand, or reuse the worker for an identical layout.
    ///
    /// Call with the output's DRM device from sleepable context, without modeset,
    /// publication or buffer reservation locks. Replacement closes the old worker
    /// before allocating its successor; allocation failure leaves no active worker.
    pub(crate) fn configure(
        &self,
        device: &Device<Driver>,
        layout: Layout,
    ) -> Result<worker::Handle> {
        self.configure_checked(device, layout, || Ok(()))
    }

    /// Revalidate a consumer after excluding other worker replacements.
    ///
    /// The callback runs before reusing or removing any worker. It may inspect display
    /// control, but must not reenter lifecycle operations or retain their outer locks.
    /// A rejected consumer leaves the current worker and its storage untouched.
    pub(crate) fn configure_checked(
        &self,
        device: &Device<Driver>,
        layout: Layout,
        check: impl FnOnce() -> Result,
    ) -> Result<worker::Handle> {
        let _change = self.lifecycle.lock();
        check()?;
        let retired = {
            let mut state = self.state.lock();
            let active = match &mut *state {
                State::Open(active) => active,
                State::Disabled => return Err(EOPNOTSUPP),
                State::Closed => return Err(ENODEV),
            };
            if let Some(current) = active.as_ref() {
                if current.layout == layout {
                    return Ok(current.worker.handle());
                }
            }
            active.take()
        };
        drop(retired);

        let pool = Pool::new(device, &self.budget, layout)?;
        let worker = worker::Owner::new(self.output.clone(), pool)?;
        let handle = worker.handle();
        // The lifecycle guard excludes replacement and shutdown until publication.
        *self.state.lock() = State::Open(Some(Active { layout, worker }));
        Ok(handle)
    }

    /// Obtain request access without allocating or waiting for configuration to finish.
    pub(crate) fn current(&self) -> Result<worker::Handle> {
        match &*self.state.lock() {
            State::Open(Some(active)) => Ok(active.worker.handle()),
            State::Open(None) => Err(EAGAIN),
            State::Disabled => Err(EOPNOTSUPP),
            State::Closed => Err(ENODEV),
        }
    }

    /// Drain and release the worker while allowing later configuration.
    ///
    /// Old handles close permanently, but output publication remains open. Retained
    /// images keep their budget charges. This controls resources only; it neither
    /// activates another executor nor changes the accepted framebuffer profile.
    /// Call outside modeset, publication and buffer reservation locks.
    pub(crate) fn stop_worker(&self) -> Result {
        let _change = self.lifecycle.lock();
        let retired = {
            let mut state = self.state.lock();
            let active = match &mut *state {
                State::Open(active) => active,
                State::Disabled => return Err(EOPNOTSUPP),
                State::Closed => return Err(ENODEV),
            };
            active.take()
        };
        drop(retired);
        Ok(())
    }

    fn close(&self) {
        let _change = self.lifecycle.lock();
        let retired = core::mem::replace(&mut *self.state.lock(), State::Closed);
        drop(retired);
    }
}

/// Unique shutdown owner; surviving configuration references cannot postpone closure.
pub(crate) struct Owner(Arc<Configuration>);

impl Owner {
    pub(crate) fn new(output: Arc<Output>) -> Result<Self> {
        let budget = Budget::new()?;
        Ok(Self(Arc::pin_init(
            pin_init!(Configuration {
                lifecycle <- kernel::new_mutex!(()),
                state <- kernel::new_mutex!(State::Open(None)),
                output,
                budget,
            }),
            GFP_KERNEL,
        )?))
    }

    pub(crate) fn configuration(&self) -> Arc<Configuration> {
        self.0.clone()
    }

    /// Close outside DRM locks; waits for configuration and queued work to finish.
    pub(crate) fn close(&self) {
        self.0.close();
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.close();
    }
}
