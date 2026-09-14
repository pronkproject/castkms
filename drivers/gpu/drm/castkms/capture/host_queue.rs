// SPDX-License-Identifier: GPL-2.0-only

//! Drive kernel HOST capture through renderer-independent client accounting.

use super::{
    destination::Image,
    host_stream::{
        output::Output,
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
    dma_fence::Fence,
    drm::capture::Status,
    prelude::*,
    sync::{
        aref::ARef,
        Arc, //
    }, //
};

enum Attempt {
    Private(Pending),
    Destination(Output),
}

impl Attempt {
    fn is_delivering(&self) -> bool {
        matches!(self, Self::Destination(output) if output.is_delivering())
    }

    fn cancel(&mut self) -> Result {
        match self {
            Self::Private(pending) => pending.cancel(),
            Self::Destination(output) => output.cancel(),
        }
    }

    fn try_complete_frame(&mut self) -> Result<Option<Frame>> {
        match self {
            Self::Private(pending) => pending.try_complete_frame(),
            Self::Destination(output) => output.try_complete_frame(),
        }
    }
}

/// Records retire before their stream; neither owns compositor-source access while waiting.
/// Operations require sleepable context outside DRM, publication and reservation locks.
pub(crate) struct Queue {
    records: requests::Queue<Attempt, Frame>,
    stream: Stream,
    closing: bool,
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
            closing: false,
        })
    }

    pub(crate) fn queue(&mut self, use_id: u64) -> Result {
        if self.closing {
            return Err(ESHUTDOWN);
        }
        self.records
            .queue(use_id, || self.stream.queue().map(Attempt::Private))
    }

    /// Retain an output attempt under the same bounded accounting as private capture.
    ///
    /// Validate the increasing use ID and reserve its terminal record before admission.
    /// Failure consumes neither the ID nor queue credit. The owner must exclude competing
    /// destination access until completion or successful try_close. Queue destruction alone
    /// abandons observation without draining detached access. Waiting for reuse does not
    /// prevent another available output from completing, nor retain a compositor-source read.
    pub(crate) fn queue_to(
        &mut self,
        use_id: u64,
        destination: Arc<Image>,
        reuse: Option<ARef<Fence>>,
    ) -> Result {
        if self.closing {
            return Err(ESHUTDOWN);
        }
        self.records.queue(use_id, || {
            self.stream
                .queue_to(destination, reuse)
                .map(Attempt::Destination)
        })
    }

    /// Request cancellation without acknowledging or replacing a terminal result.
    ///
    /// Advance the queue to observe the canceled attempt and dequeue its terminal record
    /// normally. Success is not a source-read completion or an acknowledgment of storage
    /// reuse. An already terminal record remains unchanged and returns EALREADY.
    pub(crate) fn cancel(&mut self, use_id: u64) -> Result {
        self.records.cancel(use_id, Attempt::cancel)
    }

    /// Stop new admission and request cancellation without waiting on destination access.
    ///
    /// EBUSY leaves cleanup retryable while detached access retains its storage. Success
    /// acknowledges that no accepted destination write can occur after queue destruction.
    pub(crate) fn try_close(&mut self) -> Result {
        self.closing = true;
        let mut active = false;
        self.records.for_each_pending(|attempt| {
            let _ = attempt.cancel();
            active |= attempt.is_delivering();
        });
        if active {
            Err(EBUSY)
        } else {
            Ok(())
        }
    }

    /// Observe completion and dispatch available output without entering destination exporters.
    /// Authorization and private copying may sleep; terminal errors remain accounted results.
    pub(crate) fn advance(&mut self) -> usize {
        self.records.advance(Attempt::try_complete_frame)
    }

    pub(crate) fn has_pending(&self) -> bool {
        self.records.has_pending()
    }

    pub(crate) fn has_results(&self) -> bool {
        self.records.has_results()
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
