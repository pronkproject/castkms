// SPDX-License-Identifier: GPL-2.0-only

//! Output publication and shutdown, independent of the retained scene representation.

use kernel::{
    prelude::*,
    sync::Mutex, //
};

enum Publication<S> {
    Open(Option<S>),
    Closed,
}

/// One output's committed plane description. Shutdown permanently closes publication.
///
/// The registration owner must close the output before releasing DRM registration: a scene's
/// framebuffer reference retains the DRM device, whose private data retains this output.
#[pin_data]
pub(super) struct Output<S> {
    #[pin]
    state: Mutex<Publication<S>>,
}

impl<S: Unpin> Output<S> {
    pub(super) fn new() -> impl PinInit<Self> {
        pin_init!(Self {
            state <- kernel::new_mutex!(Publication::Open(None)),
        })
    }

    pub(super) fn publish(&self, scene: Option<S>) {
        let retired = {
            let mut state = self.state.lock();
            match &mut *state {
                Publication::Open(current) => core::mem::replace(current, scene),
                Publication::Closed => scene,
            }
        };
        // Resource destruction may enter DRM; keep it outside the publication lock.
        drop(retired);
    }

    pub(super) fn close(&self) {
        let retired = {
            let mut state = self.state.lock();
            core::mem::replace(&mut *state, Publication::Closed)
        };
        drop(retired);
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
