// SPDX-License-Identifier: GPL-2.0-only

//! Explicit audio authority scoped to one master and one monitor attachment.

use super::tap::Tap;
use crate::{
    authority::grants,
    display_control::Target, //
};
use kernel::{
    drm::capture::{
        Authority,
        Creator,
        Policy as NativePolicy,
        Registration, //
    },
    prelude::*,
    sync::{
        aref::ARef,
        poll::PollCondVar,
        Arc, //
    },
    time::hrtimer::ArcHrTimerHandle, //
};

pub(super) struct Policy {
    target: Target,
    tap: Arc<Tap>,
}

// SAFETY: The vtable retains CastKMS and all callbacks until policy destruction.
#[vtable]
unsafe impl NativePolicy for Policy {
    fn revoke(&self) {
        self.tap.terminate(EKEYREVOKED);
    }
}

struct Running {
    tap: Arc<Tap>,
    timer: Option<ArcHrTimerHandle<Tap>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.tap.terminate(ECANCELED);
    }
}

/// Unique revocation duty, independent of clients and the issuing DRM file.
pub(crate) struct Owner {
    access: Access,
    _running: Running,
    creator: Option<Registration>,
    _device: grants::Registration,
}

impl Owner {
    pub(crate) fn new(target: Target) -> Result<Self> {
        // Monitor probing acquires native object-ID locks while holding its description
        // mutex. Snapshot the source before entering native display-control locks.
        let source = target.display().monitor.audio()?;
        target.with_output_objects(|| Ok(()))?;
        let tap = source.open()?;
        let running = Running {
            timer: None,
            tap: tap.clone(),
        };
        let policy = Arc::new(Policy { target, tap }, GFP_KERNEL)?;
        let authority = Authority::new(policy.clone())?;
        let device = policy
            .target
            .device()
            .audio_grants
            .register(&authority.revocation())?;
        let mut owner = Self {
            access: Access { authority, policy },
            _running: running,
            creator: None,
            _device: device,
        };
        owner.access.check()?;
        // Register revocation before starting sample reads. A master transition between
        // the final check and timer start makes the tap terminal before its first tick.
        owner._running.timer = Some(owner._running.tap.start());
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
        let result = self.policy.target.with_output_objects(|| {
            // The tap retains only its original playback. Attachment retirement makes
            // that tap terminal; no monitor lookup or rebinding is permitted here.
            if self.authority.is_revoked() {
                return Err(EKEYREVOKED);
            }
            if let Some(error) = self.policy.tap.terminal() {
                return Err(error);
            }
            Ok(f(&self.policy.tap))
        });
        match result {
            Ok(result) => result,
            Err(error) => {
                self.policy.tap.terminate(error);
                Err(error)
            }
        }
    }

    pub(crate) fn check(&self) -> Result {
        self.with_current(|_| Ok(()))
    }

    pub(crate) fn read(&self, output: &mut [u8], nonblock: bool) -> Result<usize> {
        loop {
            match self.with_current(|tap| tap.read(output)) {
                Err(EAGAIN) if !nonblock => self.policy.tap.wait()?,
                result => return result,
            }
        }
    }

    pub(super) fn changed(&self) -> &PollCondVar {
        &self.policy.tap.changed
    }

    pub(super) fn readable(&self) -> Result<bool> {
        self.with_current(|tap| Ok(tap.readable()))
    }

    pub(super) fn dropped_frames(&self) -> Result<u64> {
        self.with_current(|tap| Ok(tap.dropped()))
    }

    pub(crate) fn close(&self) {
        self.policy.tap.terminate(ECANCELED);
    }
}
