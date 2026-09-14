// SPDX-License-Identifier: GPL-2.0-only

//! Resource changes under outer lifecycle exclusion, with cleanup after display control.

use super::*;
use core::cell::RefCell;

/// Callback-local resource control, not renderer authority or framebuffer eligibility.
///
/// The wrapper owns lifecycle exclusion and keeps detached resources until this shared
/// view is gone. Callers cannot move its cleanup owner into another control operation.
pub(crate) struct Change<'a> {
    configuration: &'a Configuration,
    retired: RefCell<Option<(Active, worker::RetiredResults)>>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Configuration {
    /// Exclude replacement and shutdown before entering bounded display control.
    ///
    /// Call outside DRM, publication and reservation locks. The callback may acquire
    /// display-control locks but must release them before returning; it must not reenter
    /// configuration, shutdown, draining or this method. Detached work is drained and its
    /// images released after the callback, while lifecycle exclusion still prevents reuse.
    /// Resource changes are not rolled back if the callback reports a later error.
    pub(crate) fn with_change(&self, f: impl FnOnce(&Change<'_>) -> Result) -> Result {
        let _exclusion = self.lifecycle.lock();
        if matches!(*self.state.lock(), State::Closed) {
            return Err(ENODEV);
        }
        let change = Change {
            configuration: self,
            retired: RefCell::new(None),
        };
        let result = f(&change);
        drop(change);
        result
    }
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Change<'_> {
    /// Allow fresh lazy HOST allocation without reviving a retired worker.
    ///
    /// This changes resources only. The handback caller must independently validate the
    /// installed framebuffer's HOST eligibility and coordinate the execution profile.
    /// An already enabled worker is preserved; shutdown is never reopened.
    pub(crate) fn enable(&self) -> Result {
        let mut state = self.configuration.state.lock();
        match &*state {
            State::Disabled => *state = State::Open(None),
            State::Open(_) => (),
            State::Closed => return Err(ENODEV),
        }
        Ok(())
    }

    /// Disable lazy HOST allocation and stop the current worker's source admission.
    ///
    /// This does not wait for already admitted CPU reads or destroy their cached images.
    /// It changes resources only, not the accepted execution profile. Validate authority
    /// and the intended transition before calling; old worker handles close permanently.
    pub(crate) fn disable(&self) -> Result {
        let active = {
            let mut state = self.configuration.state.lock();
            match &mut *state {
                State::Open(active) => {
                    let retired = active.take();
                    *state = State::Disabled;
                    retired
                }
                State::Disabled => None,
                State::Closed => return Err(ENODEV),
            }
        };
        if let Some(active) = active {
            let results = active.worker.stop_admission();
            // Lifecycle exclusion prevents another worker being installed, so at most one
            // owner is detached in this callback. No earlier owner is destroyed here.
            *self.retired.borrow_mut() = Some((active, results));
        }
        Ok(())
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
