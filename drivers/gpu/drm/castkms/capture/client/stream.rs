// SPDX-License-Identifier: GPL-2.0-only

//! Borrowed stream operations without transferring the client's registered queue.

use crate::capture::{
    host_queue::Queue,
    provider::Frame,
    requests::Completion, //
};
use kernel::prelude::*;

/// Exclusive operation access while the client retains ownership of its named stream.
///
/// The view cannot replace or move the queue out of its registration. Its borrow must end
/// before the client closes that stream. Each operation retains the queue's permission,
/// acknowledgment and locking contracts; finding the stream grants no pixel authority.
pub(crate) struct Stream<'a> {
    queue: &'a mut Queue,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl<'a> Stream<'a> {
    pub(super) fn new(queue: &'a mut Queue) -> Self {
        Self { queue }
    }

    pub(crate) fn queue(&mut self, use_id: u64) -> Result {
        self.queue.queue(use_id)
    }

    pub(crate) fn advance(&mut self) -> usize {
        self.queue.advance()
    }

    pub(crate) fn dequeue<R>(
        &mut self,
        publish: impl for<'b> FnOnce(Completion<'b, Frame>) -> Result<R>,
    ) -> Result<R> {
        self.queue.dequeue(publish)
    }
}
