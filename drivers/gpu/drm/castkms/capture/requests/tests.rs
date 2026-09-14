// SPDX-License-Identifier: GPL-2.0-only

//! Client accounting is exercised without a renderer, DRM file or image allocation.

use super::*;
use core::sync::atomic::{
    AtomicU32,
    Ordering, //
};
use kernel::sync::Arc;

fn check(condition: bool) -> Result {
    if condition {
        Ok(())
    } else {
        Err(EINVAL)
    }
}

struct Tracked {
    drops: Arc<AtomicU32>,
    ready: bool,
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

#[kunit_tests(rust_castkms_capture_requests)]
mod cases {
    use super::*;

    #[test]
    fn pending_work_and_retained_results_are_independent_observations() -> Result {
        let mut queue = Queue::<bool, u32>::new(2)?;
        check(!queue.has_pending() && !queue.has_results())?;
        queue.queue(1, || Ok(false))?;
        queue.queue(2, || Ok(false))?;
        check(queue.has_pending() && !queue.has_results())?;
        queue.cancel(1, |cancelled| {
            *cancelled = true;
            Ok(())
        })?;
        check(queue.has_pending() && !queue.has_results())?;
        check(queue.advance(|cancelled| if *cancelled { Err(ECANCELED) } else { Ok(None) }) == 1)?;
        check(queue.has_pending() && queue.has_results())?;
        check(queue.dequeue::<()>(|_| Err(EFAULT)) == Err(EFAULT))?;
        check(queue.has_pending() && queue.has_results())?;
        queue.dequeue(|completion| check(matches!(completion.result, Err(ECANCELED))))?;
        check(queue.has_pending() && !queue.has_results())?;
        check(queue.advance(|_| Ok(Some(42))) == 1)?;
        check(!queue.has_pending() && queue.has_results())?;
        queue.dequeue(|completion| check(*completion.result? == 42))?;
        check(!queue.has_pending() && !queue.has_results())
    }

    #[test]
    fn cancellation_waits_for_terminal_observation_and_acknowledgment() -> Result {
        let mut queue = Queue::<bool, u32>::new(2)?;
        queue.queue(1, || Ok(false))?;
        queue.queue(2, || Ok(false))?;
        check(queue.cancel(0, |_| Err(EIO)) == Err(EINVAL))?;
        check(queue.cancel(3, |_| Err(EIO)) == Err(ENOENT))?;
        check(queue.cancel(1, |_| Err(EBUSY)) == Err(EBUSY))?;
        check(queue.advance(|_| Ok(None)) == 0)?;
        queue.cancel(1, |cancelled| {
            *cancelled = true;
            Ok(())
        })?;
        check(queue.dequeue(|_| Ok(())) == Err(EAGAIN))?;
        check(queue.queue(3, || Ok(false)) == Err(EAGAIN))?;
        check(
            queue.advance(
                |cancelled| {
                    if *cancelled {
                        Err(ECANCELED)
                    } else {
                        Ok(None)
                    }
                },
            ) == 1,
        )?;
        check(queue.cancel(1, |_| Err(EIO)) == Err(EALREADY))?;
        check(queue.dequeue::<()>(|_| Err(EFAULT)) == Err(EFAULT))?;
        check(queue.queue(3, || Ok(false)) == Err(EAGAIN))?;
        queue.dequeue(|completion| {
            check(completion.use_id == 1)?;
            check(matches!(completion.result, Err(ECANCELED)))
        })?;
        check(queue.cancel(1, |_| Err(EIO)) == Err(ENOENT))?;
        queue.queue(3, || Ok(false))?;
        check(queue.advance(|_| Ok(Some(7))) == 2)
    }

    #[test]
    fn failed_publication_retains_payloads_until_queue_close() -> Result {
        let drops = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
        let mut queue = Queue::<Tracked, Tracked>::new(2)?;
        queue.queue(1, || {
            Ok(Tracked {
                drops: drops.clone(),
                ready: false,
            })
        })?;
        queue.queue(2, || {
            Ok(Tracked {
                drops: drops.clone(),
                ready: true,
            })
        })?;
        check(
            queue.advance(|pending| {
                Ok(pending.ready.then(|| Tracked {
                    drops: drops.clone(),
                    ready: true,
                }))
            }) == 1,
        )?;
        check(drops.load(Ordering::Relaxed) == 1)?;
        check(queue.dequeue::<()>(|_| Err(EFAULT)) == Err(EFAULT))?;
        check(drops.load(Ordering::Relaxed) == 1)?;
        drop(queue);
        check(drops.load(Ordering::Relaxed) == 3)
    }

    #[test]
    fn admission_failure_does_not_burn_an_id_or_run_for_a_full_queue() -> Result {
        let mut queue = Queue::<(), u32>::new(1)?;
        check(queue.queue(1, || Err(ENOMEM)) == Err(ENOMEM))?;
        queue.queue(1, || Ok(()))?;
        let mut called = false;
        check(
            queue.queue(2, || {
                called = true;
                Ok(())
            }) == Err(EAGAIN),
        )?;
        check(!called)?;
        check(queue.advance(|_| Ok(Some(7))) == 1)?;
        queue.dequeue(|completion| check(*completion.result? == 7))?;
        check(queue.queue(1, || Ok(())) == Err(ESTALE))?;
        queue.queue(2, || Ok(()))
    }

    #[test]
    fn ready_later_requests_are_not_hidden_by_pending_earlier_requests() -> Result {
        let mut queue = Queue::<u32, u32>::new(2)?;
        queue.queue(1, || Ok(10))?;
        queue.queue(2, || Ok(20))?;
        check(queue.advance(|value| Ok((*value == 20).then_some(*value))) == 1)?;
        let fail: Result = queue.dequeue(|completion| {
            check(completion.use_id == 2)?;
            check(*completion.result? == 20)?;
            Err(EFAULT)
        });
        check(fail == Err(EFAULT))?;
        check(queue.queue(3, || Ok(30)) == Err(EAGAIN))?;
        queue.dequeue(|completion| check(completion.use_id == 2))?;
        check(queue.advance(|value| Ok(Some(*value))) == 1)?;
        queue.dequeue(|completion| {
            check(completion.use_id == 1)?;
            check(*completion.result? == 10)
        })?;
        check(queue.dequeue(|_| Ok(())) == Err(EAGAIN))
    }

    #[test]
    fn failed_operations_keep_their_records_until_acknowledged() -> Result {
        let mut queue = Queue::<(), u32>::new(1)?;
        queue.queue(u64::MAX, || Ok(()))?;
        check(queue.advance(|_| Err(EIO)) == 1)?;
        check(queue.dequeue::<()>(|_| Err(EFAULT)) == Err(EFAULT))?;
        queue.dequeue(|completion| {
            check(completion.use_id == u64::MAX)?;
            check(matches!(completion.result, Err(EIO)))
        })?;
        check(queue.queue(1, || Ok(())) == Err(EOVERFLOW))
    }
}
