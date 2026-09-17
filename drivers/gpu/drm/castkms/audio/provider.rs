// SPDX-License-Identifier: GPL-2.0-only

//! Retained audio authority with disposable per-master-interval taps.

use super::{source::Source, tap::Tap};
use crate::display_control::Target;
use kernel::{
    drm::capture::{Authority, Creator, Policy as NativePolicy, Registration},
    prelude::*,
    sync::{aref::ARef, poll::PollCondVar, Arc, Completion, Mutex},
    time::hrtimer::ArcHrTimerHandle,
};

const MAX_GRANTS: usize = 256;

struct Running {
    tap: Arc<Tap>,
    _timer: ArcHrTimerHandle<Tap>,
    interval: crate::authority::Interval,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.tap.terminate(ECANCELED);
    }
}

struct State {
    active: Option<Running>,
    closed: Option<Error>,
}

#[pin_data]
pub(super) struct Policy {
    target: Target,
    source: Arc<Source>,
    #[pin]
    state: Mutex<State>,
}

// SAFETY: The vtable retains CastKMS and all callbacks until policy destruction.
#[vtable]
unsafe impl NativePolicy for Policy {
    fn revoke(&self) {
        self.close(EKEYREVOKED);
    }
}

impl Policy {
    fn tap(&self, interval: crate::authority::Interval) -> Result<Arc<Tap>> {
        let retired = {
            let mut state = self.state.lock();
            if let Some(error) = state.closed {
                return Err(error);
            }
            if let Some(active) = &state.active {
                if active.interval == interval {
                    return Ok(active.tap.clone());
                }
            }
            state.active.take()
        };
        if let Some(retired) = retired {
            retired.tap.terminate(EAGAIN);
            drop(retired);
        }

        let tap = self.source.open()?;
        let running = Running {
            _timer: tap.start(),
            tap: tap.clone(),
            interval,
        };
        let mut state = self.state.lock();
        if let Some(error) = state.closed {
            drop(state);
            drop(running);
            return Err(error);
        }
        if state.active.is_some() {
            drop(state);
            drop(running);
            return Err(EBUSY);
        }
        state.active = Some(running);
        Ok(tap)
    }

    fn active_tap(&self) -> Option<Arc<Tap>> {
        self.state.lock().active.as_ref().map(|active| active.tap.clone())
    }

    fn suspend(&self) {
        let retired = self.state.lock().active.take();
        if let Some(retired) = retired {
            retired.tap.terminate(EAGAIN);
            drop(retired);
        }
        self.target.device().changed.notify_all();
    }

    fn close(&self, error: Error) {
        let retired = {
            let mut state = self.state.lock();
            if state.closed.is_some() {
                return;
            }
            state.closed = Some(error);
            state.active.take()
        };
        if let Some(retired) = retired {
            retired.tap.terminate(error);
            drop(retired);
        }
        self.target.device().changed.notify_all();
    }
}

/// Unique revocation duty, independent of clients and the issuing DRM file.
pub(crate) struct Owner {
    access: Access,
    creator: Option<Registration>,
    _device: DeviceRegistration,
}

impl Owner {
    pub(crate) fn new(target: Target) -> Result<Self> {
        let source = target.display().monitor.audio()?;
        let interval = target.with_output_objects(|| target.device().authority.interval())?;
        let tap = source.open()?;
        let running = Running {
            _timer: tap.start(),
            tap,
            interval,
        };
        let policy = Arc::pin_init(
            pin_init!(Policy {
                target,
                source,
                state <- kernel::new_mutex!(State { active: Some(running), closed: None }),
            }),
            GFP_KERNEL,
        )?;
        let authority = Authority::new(policy.clone())?;
        let device = policy.target.device().audio_grants.register(&policy)?;
        let owner = Self {
            access: Access { authority, policy },
            creator: None,
            _device: device,
        };
        owner.access.check()?;
        Ok(owner)
    }

    pub(crate) fn track_creator(&mut self, creator: &Creator) -> Result {
        if self.creator.is_some() {
            return Err(EEXIST);
        }
        self.creator = Some(creator.register(&self.access.authority)?);
        Ok(())
    }

    pub(crate) fn access(&self) -> Access {
        self.access.clone()
    }

    pub(super) fn authority(&self) -> ARef<Authority<Policy>> {
        self.access.authority.clone()
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.access.authority.revoke();
    }
}

/// Audio samples only: no image/source import, renderer, or monitor-control operations.
#[derive(Clone)]
pub(crate) struct Access {
    authority: ARef<Authority<Policy>>,
    policy: Arc<Policy>,
}

impl Access {
    fn with_current<T>(&self, f: impl FnOnce(&Tap) -> Result<T>) -> Result<T> {
        if self.authority.is_revoked() {
            return Err(EKEYREVOKED);
        }
        self.policy.target.with_output_objects(|| {
            if self.authority.is_revoked() {
                return Err(EKEYREVOKED);
            }
            let interval = self.policy.target.device().authority.interval()?;
            let tap = self.policy.tap(interval)?;
            if let Some(error) = tap.terminal() {
                return Err(error);
            }
            f(&tap)
        })
    }

    pub(crate) fn check(&self) -> Result {
        self.with_current(|_| Ok(())).map_err(Self::public_error)
    }

    pub(crate) fn read(&self, output: &mut [u8], nonblock: bool) -> Result<usize> {
        loop {
            match self.with_current(|tap| tap.read(output)) {
                Err(EAGAIN) if !nonblock => {
                    let Some(tap) = self.policy.active_tap() else { return Err(EAGAIN) };
                    tap.wait()?;
                }
                Err(EACCES) => return Err(EAGAIN),
                result => return result,
            }
        }
    }

    pub(super) fn active_tap(&self) -> Option<Arc<Tap>> {
        self.policy.active_tap()
    }

    pub(super) fn authority_changed(&self) -> &PollCondVar {
        &self.policy.target.device().changed
    }

    pub(super) fn readable(&self) -> Result<bool> {
        self.with_current(|tap| Ok(tap.readable())).map_err(Self::public_error)
    }

    pub(super) fn dropped_frames(&self) -> Result<u64> {
        self.with_current(|tap| Ok(tap.dropped())).map_err(Self::public_error)
    }

    pub(crate) fn close(&self) {
        self.policy.close(ECANCELED);
    }

    fn public_error(error: Error) -> Error {
        if error == EACCES { EAGAIN } else { error }
    }
}

struct RegistryState {
    closed: bool,
    policies: KVec<Arc<Policy>>,
}

#[pin_data]
pub(crate) struct Registry {
    #[pin]
    state: Mutex<RegistryState>,
    #[pin]
    cleanup_done: Completion,
}

impl Registry {
    pub(crate) fn new() -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                state <- kernel::new_mutex!(RegistryState {
                    closed: false,
                    policies: KVec::new(),
                }),
                cleanup_done <- Completion::new(),
            }),
            GFP_KERNEL,
        )
    }

    fn register(self: &Arc<Self>, policy: &Arc<Policy>) -> Result<DeviceRegistration> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(ENODEV);
        }
        if state.policies.len() == MAX_GRANTS {
            return Err(EBUSY);
        }
        state.policies.push(policy.clone(), GFP_KERNEL)?;
        Ok(DeviceRegistration { registry: self.clone(), policy: policy.clone() })
    }

    pub(crate) fn suspend_all(&self) {
        let state = self.state.lock();
        for policy in &state.policies {
            policy.suspend();
        }
    }

    pub(crate) fn close(&self) {
        let retired = {
            let mut state = self.state.lock();
            if state.closed {
                None
            } else {
                state.closed = true;
                Some(core::mem::take(&mut state.policies))
            }
        };
        if let Some(retired) = retired {
            for policy in &retired {
                policy.close(ENODEV);
            }
            drop(retired);
            self.cleanup_done.complete_all();
        } else {
            self.cleanup_done.wait_for_completion();
        }
    }

    fn remove(&self, policy: &Arc<Policy>) {
        let retired = {
            let mut state = self.state.lock();
            state.policies.iter().position(|item| Arc::ptr_eq(item, policy))
                .and_then(|index| state.policies.remove(index).ok())
        };
        drop(retired);
    }
}

struct DeviceRegistration {
    registry: Arc<Registry>,
    policy: Arc<Policy>,
}

impl Drop for DeviceRegistration {
    fn drop(&mut self) {
        self.registry.remove(&self.policy);
    }
}
