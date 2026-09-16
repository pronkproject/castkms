// SPDX-License-Identifier: GPL-2.0-only

//! Per-output worker discovery without active ownership or pixel authority.

use super::{
    candidate::Candidate,
    output_broker::{Broker, Registration},
};
use crate::{
    capture::provider::{delegated_queue::Queue, Delegated},
    output::Identity,
    renderer_startup::{Active, Observation},
};
use core::mem::MaybeUninit;
use kernel::{
    prelude::*,
    sync::{poll::PollCondVar, Arc, Mutex, UniqueArc},
};

struct State {
    closed: bool,
    current: Option<Arc<Route>>,
}

/// The registration owner closes the directory before releasing DRM registration.
/// A route retains display objects, so shutdown must break the resulting device cycle.
#[pin_data]
pub(crate) struct Registry {
    output: Identity,
    changed: Arc<PollCondVar>,
    #[pin]
    state: Mutex<State>,
}

impl Registry {
    pub(crate) fn new(output: &Identity, changed: Arc<PollCondVar>) -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                output: output.clone(),
                changed,
                state <- kernel::new_mutex!(State { closed: false, current: None }),
            }),
            GFP_KERNEL,
        )
    }

    /// Publish while the exact worker is still current. A delayed publication cannot
    /// replace a newer incarnation. Allocation is prepared before execution activation.
    pub(crate) fn publish(
        self: &Arc<Self>,
        prepared: Prepared,
        candidate: &Arc<Candidate>,
        active: &Active,
    ) -> Result<Owner> {
        let route = Arc::from(prepared.route.write(Route {
            candidate: candidate.clone(),
            active: active.observation(),
            broker: prepared.broker,
        }));
        let retired = candidate.with_observed_identity(&route.active, |output| {
            if output != &self.output {
                return Err(EINVAL);
            }
            let mut state = self.state.lock();
            if state.closed {
                return Err(ESHUTDOWN);
            }
            Ok(state.current.replace(route.clone()))
        })?;
        // Final candidate/DRM references are never released inside display or registry locks.
        drop(retired);
        self.changed.notify_all();
        Ok(Owner {
            registry: self.clone(),
            route,
        })
    }

    /// Discovery is advisory; every queue operation rechecks both authorities.
    pub(crate) fn lookup(&self) -> Result<Arc<Route>> {
        let route = {
            let state = self.state.lock();
            if state.closed {
                return Err(ESHUTDOWN);
            }
            state.current.clone().ok_or(ENODEV)?
        };
        route
            .candidate
            .with_observed_control(&route.active, |_| Ok(()))?;
        Ok(route)
    }

    pub(crate) fn close(&self) {
        let retired = {
            let mut state = self.state.lock();
            state.closed = true;
            state.current.take()
        };
        if let Some(route) = &retired {
            route.broker.close();
        }
        drop(retired);
        self.changed.notify_all();
    }
}

/// Allocate before the operation that transfers unique active-worker ownership.
pub(crate) struct Prepared {
    route: UniqueArc<MaybeUninit<Route>>,
    broker: Arc<Broker>,
}

impl Prepared {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            route: UniqueArc::new_uninit(GFP_KERNEL)?,
            broker: Broker::new()?,
        })
    }
}

/// Retained worker identity, never an active owner or a source-read grant.
pub(crate) struct Route {
    candidate: Arc<Candidate>,
    active: Observation,
    broker: Arc<Broker>,
}

impl Route {
    pub(crate) fn create_queue(&self, scope: &Delegated, capacity: u32) -> Result<Queue> {
        scope.create_queue_observed(&self.candidate, &self.active, capacity)
    }

    pub(crate) fn register_queue(&self, scope: &Delegated, capacity: u32) -> Result<Registration> {
        self.broker.register(|| self.create_queue(scope, capacity))
    }
}

/// Removal compares identity so an old endpoint cannot erase its replacement's route.
#[must_use = "dropping the owner removes its route from discovery"]
pub(crate) struct Owner {
    registry: Arc<Registry>,
    route: Arc<Route>,
}

impl Owner {
    /// Retain discovery, not endpoint ownership or permission to claim any recipient.
    pub(crate) fn broker(&self) -> Arc<Broker> {
        self.route.broker.clone()
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn try_claim(
        &self,
        image: &Arc<super::render_job::Rendered>,
    ) -> Option<super::output_broker::Job> {
        self.route.broker.try_claim(image)
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        let retired = {
            let mut state = self.registry.state.lock();
            if state
                .current
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &self.route))
            {
                state.current.take()
            } else {
                None
            }
        };
        self.route.broker.close();
        drop(retired);
        self.registry.changed.notify_all();
    }
}
