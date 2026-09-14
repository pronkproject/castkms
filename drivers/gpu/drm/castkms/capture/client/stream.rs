// SPDX-License-Identifier: GPL-2.0-only

//! Borrowed stream operations without transferring the client's registered queue.

use super::streams::Streams;
use crate::capture::{
    provider::Frame,
    requests::Completion, //
};
use kernel::prelude::*;

/// A named view while the client retains ownership of its stream.
///
/// The view cannot replace or move the queue out of its registration. Its borrow must end
/// before the client closes that stream. Each operation retains the queue's permission,
/// acknowledgment and locking contracts; finding the stream grants no pixel authority.
pub(crate) struct Stream<'a> {
    streams: &'a Streams,
    id: u64,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl<'a> Stream<'a> {
    pub(super) fn new(streams: &'a Streams, id: u64) -> Self {
        Self { streams, id }
    }

    pub(crate) fn queue(&mut self, use_id: u64) -> Result {
        self.streams
            .with_queue(self.id, |queue| queue.queue(use_id))
    }

    pub(crate) fn advance(&mut self) -> Result<usize> {
        self.streams
            .with_queue(self.id, |queue| Ok(queue.advance()))
    }

    pub(crate) fn dequeue<R>(
        &mut self,
        publish: impl for<'b> FnOnce(Completion<'b, Frame>) -> Result<R>,
    ) -> Result<R> {
        self.streams
            .with_queue(self.id, |queue| queue.dequeue(publish))
    }
}
