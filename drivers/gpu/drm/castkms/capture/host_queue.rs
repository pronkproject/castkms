// SPDX-License-Identifier: GPL-2.0-only

//! Drive kernel HOST capture through renderer-independent client accounting.

use super::{
    host_stream::{
        Pending,
        Stream, //
    },
    provider::{
        Capture,
        Description,
        Frame, //
    },
    requests::{
        self,
        Completion, //
    }, //
};
use kernel::{
    drm::capture::Status,
    prelude::*, //
};

/// Records retire before their stream; neither owns compositor-source access while waiting.
/// Operations require sleepable context outside DRM, publication and reservation locks.
pub(crate) struct Queue {
    records: requests::Queue<Pending, Frame>,
    stream: Stream,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Queue {
    pub(crate) fn new(capture: &Capture, capacity: u32) -> Result<Self> {
        Self::from_description(&capture.describe_stream()?, capacity)
    }

    /// Open the described configuration without replacing it with the latest output state.
    ///
    /// The description retains its grant, but neither permission nor storage credit.
    /// Stream creation rechecks both before starting a compositor worker. A failed
    /// opening leaves the description available for an explicit retry.
    pub(crate) fn from_description(description: &Description, capacity: u32) -> Result<Self> {
        // Stream creation enforces the provider's capacity limit before allocating records.
        let stream = Stream::from_description(description, capacity)?;
        Ok(Self {
            records: requests::Queue::new(capacity as usize)?,
            stream,
        })
    }

    pub(crate) fn queue(&mut self, use_id: u64) -> Result {
        self.records.queue(use_id, || self.stream.queue())
    }

    /// Finish available composition without waiting for the worker. Authorization and copying
    /// may sleep; every error becomes a retained terminal result rather than lost demand.
    pub(crate) fn advance(&mut self) -> usize {
        self.records.advance(Pending::try_complete_frame)
    }

    /// Publish a terminal record without returning its credit on a failed output operation.
    ///
    /// Native result status is rechecked before exposing a frame; stream shutdown may discard
    /// stored pixels independently of this queue. The callback must finish all fallible output
    /// work before returning success, and cannot retain a borrow past acknowledgment.
    pub(crate) fn dequeue<R>(
        &mut self,
        publish: impl for<'a> FnOnce(Completion<'a, Frame>) -> Result<R>,
    ) -> Result<R> {
        self.records.dequeue(|completion| {
            let result = completion
                .result
                .and_then(|frame| match frame.request().status()? {
                    Status::Complete(result) => result.map(|()| frame),
                    Status::Pending => Err(EIO),
                });
            publish(Completion {
                use_id: completion.use_id,
                result,
            })
        })
    }
}
