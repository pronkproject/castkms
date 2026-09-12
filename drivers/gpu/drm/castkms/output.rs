// SPDX-License-Identifier: GPL-2.0-only

//! Output publication and shutdown, independent of the retained scene representation.

mod cpu_read;

use kernel::{
    drm::preparation::Source,
    prelude::*,
    sync::{
        aref::ARef,
        Mutex, //
    }, //
};

/// Whether the accepted transaction supplied a new primary-plane description.
pub(super) enum SceneUpdate<S> {
    Retain,
    Replace(Option<S>),
}

/// Accounting and the image it describes are published as one indivisible generation.
struct Generation<S> {
    source: ARef<Source>,
    scene: Option<S>,
}

impl<S> Drop for Generation<S> {
    fn drop(&mut self) {
        self.source.seal();
    }
}

enum Publication<S> {
    Open(Option<Generation<S>>),
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

    /// Publish a fresh accepted generation, including updates without a primary image.
    ///
    /// The source belongs to the new accepted CRTC state, not a preceding publication.
    pub(super) fn publish(&self, source: ARef<Source>, update: SceneUpdate<S>) {
        let retain = matches!(update, SceneUpdate::Retain);
        let mut next = Generation {
            source,
            scene: match update {
                SceneUpdate::Retain => None,
                SceneUpdate::Replace(scene) => scene,
            },
        };
        let retired = {
            let mut state = self.state.lock();
            match &mut *state {
                Publication::Open(current) => {
                    if retain {
                        next.scene = current.as_mut().and_then(|current| current.scene.take());
                    }
                    current.replace(next)
                }
                Publication::Closed => Some(next),
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

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) fn inspect<R>(&self, inspect: impl FnOnce(Option<&S>) -> R) -> R {
        self.inspect_accepted(|accepted| inspect(accepted.and_then(|(_, scene)| scene)))
    }

    #[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
    pub(super) fn inspect_accepted<R>(
        &self,
        inspect: impl FnOnce(Option<(&Source, Option<&S>)>) -> R,
    ) -> R {
        let state = self.state.lock();
        match &*state {
            Publication::Open(current) => inspect(
                current
                    .as_ref()
                    .map(|item| (&*item.source, item.scene.as_ref())),
            ),
            Publication::Closed => inspect(None),
        }
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
