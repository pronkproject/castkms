// SPDX-License-Identifier: GPL-2.0-only

//! Charged native result storage, retained by streams and returned requests.

use super::Policy;
use crate::capture::budget::Charge;
use kernel::{
    drm::capture::{
        Authority,
        Stream, //
    },
    prelude::*,
    sync::{
        aref::ARef,
        Arc, //
    }, //
};

/// Field order releases native storage before its reservation, then provider/module ownership.
pub(super) struct Storage {
    pub(super) native: ARef<Stream>,
    _charge: Charge,
    _authority: ARef<Authority<Policy>>,
}

impl Storage {
    pub(super) fn new(charge: Charge, authority: ARef<Authority<Policy>>) -> Result<Arc<Self>> {
        Ok(Arc::new(
            Self {
                native: Stream::new(charge.capacity(), charge.layout().pixel_bytes())?,
                _charge: charge,
                _authority: authority,
            },
            GFP_KERNEL,
        )?)
    }
}
