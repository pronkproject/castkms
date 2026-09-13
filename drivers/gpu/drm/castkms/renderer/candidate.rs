// SPDX-License-Identifier: GPL-2.0-only

//! Authorized private startup for one accepted display configuration.

use super::permission::Access;
use crate::{
    execution::{
        self,
        Description, //
    },
    renderer_startup,
    scene::Configuration, //
};
use kernel::prelude::*;

/// One renderer's private reservation, without an activation or live-source claim.
///
/// Configuration equality denotes the same mode and route interval, not equal dimensions.
/// Content-only updates do not invalidate startup. The access handle retains its issuer's
/// revocation state, not its issuer lifetime. Drop outside native DRM and admission locks.
#[must_use = "dropping the candidate releases its private startup reservation"]
pub(crate) struct Candidate {
    resources: renderer_startup::Candidate,
    access: Access,
    configuration: Configuration,
    execution: Description,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Candidate {
    /// Reserve outside policy locks, checking the configuration on both sides.
    pub(crate) fn begin(access: Access) -> Result<Self> {
        Self::begin_then(access, || Ok(()))
    }

    fn begin_then(access: Access, after_reserve: impl FnOnce() -> Result) -> Result<Self> {
        let configuration = access.with_current(|current| Ok(current.configuration().clone()))?;
        let execution = execution::describe();
        let resources = access.device().startup.begin()?;
        let candidate = Self {
            resources,
            access,
            configuration,
            execution,
        };
        after_reserve()?;
        candidate.validate()?;
        Ok(candidate)
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(crate) fn begin_then_for_test(
        access: Access,
        after_reserve: impl FnOnce() -> Result,
    ) -> Result<Self> {
        Self::begin_then(access, after_reserve)
    }

    /// Historical configuration metadata, not permission to activate or read pixels.
    pub(crate) fn configuration(&self) -> &Configuration {
        &self.configuration
    }

    /// Recheck authority and private reservation without changing active execution.
    ///
    /// Success is an observation only. A later operation must perform its own validation
    /// and admission under the locks governing that operation.
    pub(crate) fn validate(&self) -> Result {
        self.access.with_current(|current| {
            if current.configuration() != &self.configuration
                || execution::describe() != self.execution
            {
                return Err(ESTALE);
            }
            self.resources.check()
        })
    }

    /// Cancel only this reservation; retained objects cannot cancel its replacement.
    pub(crate) fn cancel(&self) {
        self.resources.cancel();
    }
}
