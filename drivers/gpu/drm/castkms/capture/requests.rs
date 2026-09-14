// SPDX-License-Identifier: GPL-2.0-only

//! Bounded client accounting, independent of rendering and descriptor transport.

use kernel::prelude::*;

enum State<P, T> {
    Pending(P),
    Ready(Result<T>),
}

struct Record<P, T> {
    use_id: u64,
    state: State<P, T>,
}

/// A borrowed terminal result, without any implied pixel-access permission.
///
/// This observation installs no descriptor and acknowledges nothing by itself.
pub(crate) struct Completion<'a, T> {
    pub(crate) use_id: u64,
    pub(crate) result: Result<&'a T>,
}

/// One stream incarnation with bounded demand and unacknowledged terminal records.
///
/// Client IDs increase strictly and never become available again after acknowledgment.
/// Failed attempts occupy accounting credit until dequeued, even if their pixel storage
/// has already been released. The adapter owns resource admission, source lifetimes and
/// payload validity. Drop abandons records; payload owners must preserve any active access.
pub(crate) struct Queue<P, T> {
    records: KVec<Record<P, T>>,
    limit: usize,
    last_use: u64,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl<P, T> Queue<P, T> {
    pub(crate) fn new(capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(EINVAL);
        }
        Ok(Self {
            records: KVec::with_capacity(capacity, GFP_KERNEL)?,
            limit: capacity,
            last_use: 0,
        })
    }

    /// Admit a pending operation only after validating its ID and reserving its terminal record.
    ///
    /// Zero is invalid. Once u64::MAX has been accepted, a new stream incarnation is needed.
    /// Rejection leaves the high-water mark unchanged, so an unaccepted ID remains retryable.
    pub(crate) fn queue(&mut self, use_id: u64, admit: impl FnOnce() -> Result<P>) -> Result {
        if use_id == 0 {
            return Err(EINVAL);
        }
        if self.last_use == u64::MAX {
            return Err(EOVERFLOW);
        }
        if use_id <= self.last_use {
            return Err(ESTALE);
        }
        if self.records.len() == self.limit {
            return Err(EAGAIN);
        }
        let pending = admit()?;
        self.records.push(
            Record {
                use_id,
                state: State::Pending(pending),
            },
            GFP_KERNEL,
        )?;
        self.last_use = use_id;
        Ok(())
    }

    /// Observe pending operations once each, retaining every terminal outcome.
    ///
    /// The adapter decides whether observation waits or accesses pixels. Errors occupy
    /// terminal accounting just like successful payloads, until acknowledged or abandoned.
    pub(crate) fn advance(
        &mut self,
        mut observe: impl FnMut(&mut P) -> Result<Option<T>>,
    ) -> usize {
        let mut completed = 0;
        for record in &mut self.records {
            let State::Pending(pending) = &mut record.state else {
                continue;
            };
            let result = match observe(pending) {
                Ok(None) => continue,
                Ok(Some(value)) => Ok(value),
                Err(error) => Err(error),
            };
            record.state = State::Ready(result);
            completed += 1;
        }
        completed
    }

    /// Publish one ready result and acknowledge only after the callback succeeds.
    ///
    /// An error, including a userspace copy fault, leaves the exact record retryable. The
    /// callback must complete all fallible output work before reporting success. Its borrows
    /// cannot escape acknowledgment. Pending earlier requests do not hide ready later ones.
    /// This method does not drive composition or authorize additional pixel access.
    pub(crate) fn dequeue<R>(
        &mut self,
        publish: impl for<'a> FnOnce(Completion<'a, T>) -> Result<R>,
    ) -> Result<R> {
        let index = self
            .records
            .iter()
            .position(|record| matches!(record.state, State::Ready(_)))
            .ok_or(EAGAIN)?;
        let record = &self.records[index];
        let State::Ready(result) = &record.state else {
            return Err(EIO);
        };
        let published = publish(Completion {
            use_id: record.use_id,
            result: result.as_ref().map_err(|error| *error),
        })?;
        // The exclusive queue borrow keeps the selected index present across publication.
        drop(self.records.remove(index).map_err(|_| EIO)?);
        Ok(published)
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
