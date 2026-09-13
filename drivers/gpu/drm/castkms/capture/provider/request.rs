// SPDX-License-Identifier: GPL-2.0-only

//! Result observation retaining its native stream and reserved private storage.

use super::storage::Storage;
use kernel::{
    drm::capture::{
        Request as NativeRequest,
        Status, //
    },
    prelude::*,
    sync::Arc, //
};

/// Drop the native request before releasing the last possible owner of its storage charge.
#[must_use = "dropping a request abandons demand or its retained result"]
pub(crate) struct Request {
    native: NativeRequest,
    _storage: Arc<Storage>,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Request {
    pub(super) fn new(storage: &Arc<Storage>) -> Result<Self> {
        Ok(Self {
            native: storage.native.queue()?,
            _storage: storage.clone(),
        })
    }

    pub(crate) fn status(&self) -> Result<Status> {
        self.native.status()
    }

    /// The outer result describes waiting; the inner result describes capture validity.
    pub(crate) fn wait(&self) -> Result<Result> {
        self.native.wait()
    }

    pub(crate) fn copy_result(&self, pixels: &mut [u8]) -> Result<usize> {
        self.native.copy_result(pixels)
    }

    pub(crate) fn cancel(&self) -> Result {
        self.native.cancel()
    }
}
