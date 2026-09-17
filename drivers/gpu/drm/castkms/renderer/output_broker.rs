// SPDX-License-Identifier: GPL-2.0-only

//! Bounded discovery of recipient demand by one renderer worker.

use super::render_job::Rendered;
use crate::capture::{
    provider::delegated_queue::{Job as OutputJob, Queue},
    request_budget::QUEUE_LIMIT,
};
use core::sync::atomic::{AtomicBool, Ordering};
use kernel::{
    prelude::*,
    sync::{Arc, Mutex},
};

#[pin_data]
struct Endpoint {
    closing: AtomicBool,
    #[pin]
    queue: Mutex<Queue>,
}

impl Endpoint {
    /// Stop discovery without waiting behind a recipient's faulting metadata copy.
    fn request_close(&self) {
        self.closing.store(true, Ordering::SeqCst);
        self.finish_close();
    }

    /// Every queue operation rechecks after unlocking. Either the closer acquires
    /// exclusion here or its current holder observes closure in its own epilogue.
    fn finish_close(&self) {
        if self.closing.load(Ordering::SeqCst) {
            if let Some(mut queue) = self.queue.try_lock() {
                let _ = queue.try_close();
            }
        }
    }
}

struct Entry {
    id: u64,
    endpoint: Arc<Endpoint>,
}

struct State {
    entries: [Option<Entry>; QUEUE_LIMIT],
    next_id: u64,
    cursor: usize,
    closed: bool,
}

impl State {
    fn admission(&self) -> Result<(usize, u64)> {
        if self.closed {
            return Err(ESHUTDOWN);
        }
        self.next_id.checked_add(1).ok_or(EOVERFLOW)?;
        let slot = self.entries.iter().position(Option::is_none).ok_or(EBUSY)?;
        Ok((slot, self.next_id))
    }
}

/// Owns no active renderer identity. Queues perform exact worker/recipient checks.
/// Closing or dropping a recipient removes demand, not outstanding native accesses.
#[pin_data]
pub(crate) struct Broker {
    #[pin]
    state: Mutex<State>,
}

impl Broker {
    pub(crate) fn new() -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                state <- kernel::new_mutex!(State {
                    entries: core::array::from_fn(|_| None),
                    next_id: 1,
                    cursor: 0,
                    closed: false,
                }),
            }),
            GFP_KERNEL,
        )
    }

    /// Create outside the directory lock, then recheck bounded publication.
    /// Failure consumes no name and destroys constructed queues outside exclusion.
    pub(crate) fn register(
        self: &Arc<Self>,
        create: impl FnOnce() -> Result<Queue>,
    ) -> Result<Registration> {
        self.state.lock().admission()?;
        let endpoint = Arc::pin_init(
            try_pin_init!(Endpoint {
                closing: AtomicBool::new(false),
                queue <- kernel::new_mutex!(create()?),
            }),
            GFP_KERNEL,
        )?;
        let id = {
            let mut state = self.state.lock();
            let (slot, id) = state.admission()?;
            state.entries[slot] = Some(Entry {
                id,
                endpoint: endpoint.clone(),
            });
            state.next_id = id + 1;
            id
        };
        Ok(Registration {
            broker: self.clone(),
            id,
            endpoint,
        })
    }

    /// Scan each recipient once without holding directory exclusion during admission.
    /// Round-robin selection prevents one repeatedly ready recipient from monopolizing E.
    pub(crate) fn try_claim(&self, image: &Arc<Rendered>) -> Option<Job> {
        let snapshot: [Option<(usize, Arc<Endpoint>)>; QUEUE_LIMIT] = {
            let state = self.state.lock();
            if state.closed {
                return None;
            }
            core::array::from_fn(|offset| {
                let slot = (state.cursor + offset) % QUEUE_LIMIT;
                state.entries[slot]
                    .as_ref()
                    .map(|entry| (slot, entry.endpoint.clone()))
            })
        };
        for (slot, endpoint) in snapshot.into_iter().flatten() {
            // Recipient metadata publication may fault in userspace. Never wait behind
            // that client while selecting output for the renderer's independent stages.
            let claimed = match endpoint.queue.try_lock() {
                Some(mut queue) => {
                    if endpoint.closing.load(Ordering::SeqCst) {
                        let _ = queue.try_close();
                        None
                    } else {
                        queue.try_claim(image)
                    }
                }
                None => continue,
            };
            endpoint.finish_close();
            if let Some(output) = claimed {
                self.state.lock().cursor = (slot + 1) % QUEUE_LIMIT;
                return Some(Job { output });
            }
        }
        None
    }

    pub(crate) fn can_claim(&self, image: &Rendered) -> bool {
        let endpoints: [Option<Arc<Endpoint>>; QUEUE_LIMIT] = {
            let state = self.state.lock();
            if state.closed { return false; }
            core::array::from_fn(|offset| {
                let slot = (state.cursor + offset) % QUEUE_LIMIT;
                state.entries[slot].as_ref().map(|entry| entry.endpoint.clone())
            })
        };
        for endpoint in endpoints.into_iter().flatten() {
            let ready = endpoint.queue.try_lock().is_some_and(|mut queue| {
                !endpoint.closing.load(Ordering::SeqCst) && queue.can_claim(image)
            });
            endpoint.finish_close();
            if ready { return true; }
        }
        false
    }

    /// Stop discovery and cancel demand without waiting for native completion.
    pub(crate) fn close(&self) {
        let retired = {
            let mut state = self.state.lock();
            state.closed = true;
            core::mem::replace(&mut state.entries, core::array::from_fn(|_| None))
        };
        for entry in retired.into_iter().flatten() {
            entry.endpoint.request_close();
        }
    }
}

/// The exact recipient queue is named separately from its local destination-use IDs.
pub(crate) struct Job {
    pub(crate) output: OutputJob,
}

/// Unique recipient registration. Observers and native work do not keep it discoverable.
#[must_use = "dropping registration cancels its remaining demand"]
pub(crate) struct Registration {
    broker: Arc<Broker>,
    id: u64,
    endpoint: Arc<Endpoint>,
}

impl Registration {
    /// The callback may publish metadata but must not reenter this queue's operations.
    pub(crate) fn with_queue<R>(
        &self,
        operation: impl FnOnce(&mut Queue) -> Result<R>,
    ) -> Result<R> {
        let (changed, result) = {
            let mut queue = self.endpoint.queue.lock();
            if self.endpoint.closing.load(Ordering::SeqCst) {
                let _ = queue.try_close();
            }
            let changed = queue.changed().clone();
            (changed, operation(&mut queue))
        };
        self.endpoint.finish_close();
        // A worker may have observed an in-lock notification and skipped this busy
        // recipient. Notify after unlocking so accepted demand cannot lose its wakeup.
        changed.notify_all();
        result
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let retired = {
            let mut state = self.broker.state.lock();
            state
                .entries
                .iter_mut()
                .find(|entry| entry.as_ref().is_some_and(|entry| entry.id == self.id))
                .and_then(Option::take)
        };
        self.endpoint.request_close();
        drop(retired);
    }
}
