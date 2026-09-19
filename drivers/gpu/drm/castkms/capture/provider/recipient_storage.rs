// SPDX-License-Identifier: GPL-2.0-only

//! Known HOST recipient backing shares the private/output alias ledger.

use super::Capture;
use crate::{
    capture::{destination::Image, output_layout::Layout},
    image_storage::Pool,
};
use kernel::prelude::*;

impl Capture {
    /// Account for registered HOST destinations without mapping or claiming access.
    /// Allocation and final-reference release run outside display and authority guards.
    pub(crate) fn retain_destination_storage(&self, image: &mut Image) -> Result {
        if image.layout() != Layout::HOST {
            return Err(EOPNOTSUPP);
        }
        self.describe_stream()?;
        let storage = self.policy.permission.device().image_storage.register(
            Pool::Recipient,
            image.dimensions(),
            &[image.buffer().into()],
        )?;
        self.describe_stream()?;
        image.retain_storage(storage)
    }
}
