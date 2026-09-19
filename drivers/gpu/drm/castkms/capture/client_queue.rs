// SPDX-License-Identifier: GPL-2.0-only

//! One capture-client stream over either host or delegated execution.

use super::{destination::Image, host_queue, provider::{delegated_queue, Delegated}};
use crate::capture::output_layout::Layout;
use kernel::{dma_fence::Fence, prelude::*, sync::{aref::ARef, Arc}, time::{Instant, Monotonic}};

pub(crate) struct Completion {
    pub(crate) use_id: u64,
    pub(crate) result: Result<Output>,
}

pub(crate) struct Output {
    completed_at: Instant<Monotonic>,
}

impl Output {
    pub(crate) fn metadata(&self) -> &Self { self }
    pub(crate) fn completed_at(&self) -> Instant<Monotonic> { self.completed_at }
}

pub(crate) enum Queue {
    Host(host_queue::Queue),
    Delegated {
        registration: crate::renderer::output_broker::Registration,
        layout: Layout,
    },
}

impl Queue {
    pub(crate) fn delegated(scope: &Delegated, layout: Layout, capacity: u32) -> Result<Self> {
        Ok(Self::Delegated {
            registration: scope.register_queue(capacity)?,
            layout,
        })
    }

    pub(crate) fn queue(&mut self, use_id: u64) -> Result {
        match self {
            Self::Host(queue) => queue.queue(use_id),
            Self::Delegated { .. } => Err(EOPNOTSUPP),
        }
    }

    pub(crate) fn queue_to(
        &mut self,
        use_id: u64,
        destination: Arc<Image>,
        reuse: Option<ARef<Fence>>,
    ) -> Result {
        match self {
            Self::Host(queue) => queue.queue_to(use_id, destination, reuse),
            Self::Delegated { registration, layout } => {
                if destination.layout() != *layout { return Err(EINVAL); }
                let destination = destination.delegated()?;
                registration.with_queue(|queue| queue.queue_to(use_id, &destination, reuse))
            }
        }
    }

    pub(crate) fn cancel(&mut self, use_id: u64) -> Result {
        match self {
            Self::Host(queue) => queue.cancel(use_id),
            Self::Delegated { registration, .. } => {
                registration.with_queue(|queue| queue.cancel(use_id))
            }
        }
    }

    pub(crate) fn try_close(&mut self) -> Result {
        match self {
            Self::Host(queue) => queue.try_close(),
            Self::Delegated { registration, .. } => {
                registration.with_queue(delegated_queue::Queue::try_close)
            }
        }
    }

    pub(crate) fn advance(&mut self) -> usize {
        match self {
            Self::Host(queue) => queue.advance(),
            Self::Delegated { registration, .. } => registration
                .with_queue(|queue| Ok(queue.advance()))
                .unwrap_or(0),
        }
    }

    pub(crate) fn has_pending(&self) -> bool {
        match self {
            Self::Host(queue) => queue.has_pending(),
            Self::Delegated { registration, .. } => registration
                .with_queue(|queue| Ok(queue.has_pending()))
                .unwrap_or(false),
        }
    }

    pub(crate) fn has_results(&self) -> bool {
        match self {
            Self::Host(queue) => queue.has_results(),
            Self::Delegated { registration, .. } => registration
                .with_queue(|queue| Ok(queue.has_results()))
                .unwrap_or(false),
        }
    }

    pub(crate) fn dequeue<R>(
        &mut self,
        publish: impl FnOnce(Completion) -> Result<R>,
    ) -> Result<R> {
        match self {
            Self::Host(queue) => queue.dequeue(|completion| publish(Completion {
                use_id: completion.use_id,
                result: completion.result.map(|frame| Output {
                    completed_at: frame.metadata().completed_at(),
                }),
            })),
            Self::Delegated { registration, .. } => registration.with_queue(|queue| {
                queue.dequeue(|completion| {
                    publish(Completion {
                        use_id: completion.use_id,
                        result: completion.result.and_then(|()| {
                            Ok(Output {
                                completed_at: completion.completed_at.ok_or(EIO)?,
                            })
                        }),
                    })
                })
            }),
        }
    }
}
