// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::drm::capture::ClientStream;
use crate::sync::Arc;
use core::cell::Cell;
use core::sync::atomic::{
    AtomicBool,
    AtomicU32,
    Ordering, //
};

#[derive(Default)]
struct Counts {
    revokes: AtomicU32,
    owners: AtomicU32,
    policies: AtomicU32,
    streams_opened: AtomicU32,
    streams_closed: AtomicU32,
    fail_close: AtomicBool,
}

struct TestOwner(Arc<Counts>);

struct NotifyingOwner {
    readiness: ARef<Readiness>,
    counts: Arc<Counts>,
}

impl Drop for NotifyingOwner {
    fn drop(&mut self) {
        self.counts.owners.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: Notification callback and owner destruction belong to LocalModule.
#[vtable]
unsafe impl ClientOwner for NotifyingOwner {
    fn readiness(&self) -> Option<&Readiness> {
        Some(&self.readiness)
    }
}

struct StreamOwner {
    counts: Arc<Counts>,
    active: Option<u64>,
    last: u64,
}

// SAFETY: Stream callbacks and their owner destruction belong to LocalModule.
#[vtable]
unsafe impl ClientOwner for StreamOwner {
    fn open_stream(&mut self, id: u64, offer: u64, capacity: u32) -> Result {
        if offer != 19 || capacity != 2 {
            return Err(EINVAL);
        }
        if id <= self.last {
            return Err(ESTALE);
        }
        if self.active.is_some() {
            return Err(EBUSY);
        }
        self.active = Some(id);
        self.last = id;
        self.counts.streams_opened.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn close_stream(&mut self, id: u64) -> Result {
        if self.counts.fail_close.swap(false, Ordering::Relaxed) {
            return Err(EIO);
        }
        if self.active != Some(id) {
            return Err(ENOENT);
        }
        self.active = None;
        self.counts.streams_closed.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

struct DescribingOwner(Cell<u64>);

// SAFETY: The callback and owner destruction belong to LocalModule.
#[vtable]
unsafe impl ClientOwner for DescribingOwner {
    fn describe(&mut self) -> Result<Description> {
        let id = self.0.get().checked_add(1).ok_or(EOVERFLOW)?;
        self.0.set(id);
        Description::new(
            id,
            [64, 32],
            60_000,
            0,
            crate::drm::fourcc::XRGB8888,
            0,
            8,
        )
    }
}

// SAFETY: The owner's release trampoline and destructor belong to LocalModule.
#[vtable]
unsafe impl ClientOwner for TestOwner {}

impl Drop for TestOwner {
    fn drop(&mut self) {
        self.0.owners.fetch_add(1, Ordering::Relaxed);
    }
}

struct TestPolicy(Arc<Counts>);

// SAFETY: The policy's callbacks and destructor belong to LocalModule.
#[vtable]
unsafe impl Policy for TestPolicy {
    fn revoke(&self) {
        self.0.revokes.fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for TestPolicy {
    fn drop(&mut self) {
        self.0.policies.fetch_add(1, Ordering::Relaxed);
    }
}

fn release(file: ARef<File>) {
    // SAFETY: KUnit runs in a kernel thread. Transfer its owned reference and finish final
    // release synchronously before inspecting the destruction counts.
    unsafe { bindings::__fput_sync(ARef::into_raw(file).cast().as_ptr()) };
}

fn check(condition: bool) -> Result {
    if condition {
        Ok(())
    } else {
        Err(EINVAL)
    }
}

#[kunit_tests(rust_drm_capture_client_file)]
mod cases {
    use super::*;

    #[test]
    fn notification_survives_provider_and_file_release() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = Authority::new(Arc::new(TestPolicy(counts.clone()), GFP_KERNEL)?)?;
        let readiness = Readiness::new()?;
        let file = authority.create_client_file(NotifyingOwner {
            readiness: readiness.clone(),
            counts: counts.clone(),
        })?;
        let retained = Readiness::for_client(&file)?;
        check(!retained.has_results())?;
        readiness.update(true);
        check(retained.has_results())?;
        authority.revoke();
        check(Readiness::for_client(&file)?.has_results())?;
        release(file);
        check(counts.owners.load(Ordering::Relaxed) == 1)?;
        drop(readiness);
        retained.update(false);
        check(!retained.has_results())
    }

    #[test]
    fn notification_lookup_rejects_other_roles_and_absent_support() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = Authority::new(Arc::new(TestPolicy(counts.clone()), GFP_KERNEL)?)?;
        let client = authority.create_client_file(TestOwner(counts.clone()))?;
        let control = authority.create_control_file()?;
        let absent = Readiness::for_client(&client).err();
        let wrong_role = Readiness::for_client(&control).err();
        release(client);
        release(control);
        check(absent == Some(EOPNOTSUPP))?;
        check(wrong_role == Some(EINVAL))
    }

    #[test]
    fn owned_stream_drop_closes_after_revocation_without_revoking_its_client() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = Authority::new(Arc::new(TestPolicy(counts.clone()), GFP_KERNEL)?)?;
        let file = authority.create_client_file(StreamOwner {
            counts: counts.clone(),
            active: None,
            last: 0,
        })?;
        let stream = ClientStream::open(&file, 7, 19, 2)?;
        check(stream.id() == Some(7))?;
        check(counts.streams_opened.load(Ordering::Relaxed) == 1)?;
        drop(stream);
        check(!authority.is_revoked())?;
        let mut stream = ClientStream::open(&file, 8, 19, 2)?;
        // The stream retains the client after its caller releases the original reference.
        release(file);
        authority.revoke();
        stream.close()?;
        drop(stream);
        check(counts.streams_closed.load(Ordering::Relaxed) == 2)?;
        check(counts.revokes.load(Ordering::Relaxed) == 1)?;
        Ok(())
    }

    #[test]
    fn external_close_does_not_let_an_old_handle_close_a_later_stream() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = Authority::new(Arc::new(TestPolicy(counts.clone()), GFP_KERNEL)?)?;
        let file = authority.create_client_file(StreamOwner {
            counts: counts.clone(),
            active: None,
            last: 0,
        })?;
        let mut old = ClientStream::open(&file, 1, 19, 2)?;
        // SAFETY: The owned client file remains live while another holder's explicit close
        // is simulated through native role-checked dispatch. No pointer ownership transfers.
        check(unsafe { bindings::drm_capture_client_close_stream(file.as_ptr(), 1) } == 0)?;
        let next = ClientStream::open(&file, 2, 19, 2)?;
        old.close()?;
        check(old.id().is_none())?;
        drop(old);
        check(counts.streams_closed.load(Ordering::Relaxed) == 1)?;
        drop(next);
        check(counts.streams_closed.load(Ordering::Relaxed) == 2)?;
        release(file);
        Ok(())
    }

    #[test]
    fn explicit_stream_close_retains_failure_for_retry_and_is_idempotent() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = Authority::new(Arc::new(TestPolicy(counts.clone()), GFP_KERNEL)?)?;
        let file = authority.create_client_file(StreamOwner {
            counts: counts.clone(),
            active: None,
            last: 0,
        })?;
        let mut stream = ClientStream::open(&file, 1, 19, 2)?;
        counts.fail_close.store(true, Ordering::Relaxed);
        check(stream.close() == Err(EIO))?;
        check(stream.id() == Some(1))?;
        check(matches!(ClientStream::open(&file, 2, 19, 2), Err(EBUSY)))?;
        stream.close()?;
        stream.close()?;
        check(stream.id().is_none())?;
        drop(stream);
        check(counts.streams_closed.load(Ordering::Relaxed) == 1)?;
        check(!authority.is_revoked())?;
        check(matches!(ClientStream::open(&file, 1, 19, 2), Err(ESTALE)))?;
        drop(ClientStream::open(&file, 2, 19, 2)?);
        check(counts.streams_closed.load(Ordering::Relaxed) == 2)?;
        release(file);
        Ok(())
    }

    #[test]
    fn stream_handles_reject_invalid_and_unsupported_endpoints() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = Authority::new(Arc::new(TestPolicy(counts.clone()), GFP_KERNEL)?)?;
        let file = authority.create_client_file(TestOwner(counts.clone()))?;
        check(matches!(ClientStream::open(&file, 0, 19, 2), Err(EINVAL)))?;
        check(matches!(
            ClientStream::open(&file, 1, 19, 2),
            Err(EOPNOTSUPP)
        ))?;
        let control = authority.create_control_file()?;
        check(matches!(
            ClientStream::open(&control, 1, 19, 2),
            Err(EINVAL)
        ))?;
        authority.revoke();
        check(matches!(
            ClientStream::open(&file, 1, 19, 2),
            Err(EKEYREVOKED)
        ))?;
        release(file);
        release(control);
        Ok(())
    }

    #[test]
    fn queries_exclusively_borrow_a_send_only_owner() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = Authority::new(Arc::new(TestPolicy(counts.clone()), GFP_KERNEL)?)?;
        let file = authority.create_client_file(DescribingOwner(Cell::new(0)))?;
        check(Description::query(&file)?.id() == 1)?;
        let next = Description::query(&file)?;
        check(next.id() == 2 && next.dimensions() == [64, 32])?;
        check(next.max_requests() == 8)?;
        authority.revoke();
        check(Description::query(&file) == Err(EKEYREVOKED))?;
        release(file);
        Ok(())
    }

    #[test]
    fn absent_description_and_revocation_file_are_not_providers() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = Authority::new(Arc::new(TestPolicy(counts.clone()), GFP_KERNEL)?)?;
        let client = authority.create_client_file(TestOwner(counts.clone()))?;
        check(Description::query(&client) == Err(EOPNOTSUPP))?;
        let control = authority.create_control_file()?;
        check(Description::query(&control) == Err(EINVAL))?;
        release(client);
        release(control);
        Ok(())
    }

    #[test]
    fn client_close_releases_only_its_own_owner() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = Authority::new(Arc::new(TestPolicy(counts.clone()), GFP_KERNEL)?)?;
        let first = authority.create_client_file(TestOwner(counts.clone()))?;
        let second = authority.create_client_file(TestOwner(counts.clone()))?;
        release(first.clone());
        check(counts.owners.load(Ordering::Relaxed) == 0)?;
        release(first);
        check(counts.owners.load(Ordering::Relaxed) == 1)?;
        check(!authority.is_revoked())?;
        drop(authority.begin()?);
        release(second);
        check(counts.owners.load(Ordering::Relaxed) == 2)?;
        check(counts.revokes.load(Ordering::Relaxed) == 0)?;
        drop(authority);
        check(counts.revokes.load(Ordering::Relaxed) == 1)?;
        check(counts.policies.load(Ordering::Relaxed) == 1)
    }

    #[test]
    fn last_client_reference_retains_normal_authority_cleanup() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = Authority::new(Arc::new(TestPolicy(counts.clone()), GFP_KERNEL)?)?;
        let file = authority.create_client_file(TestOwner(counts.clone()))?;
        drop(authority);
        check(counts.revokes.load(Ordering::Relaxed) == 0)?;
        check(counts.policies.load(Ordering::Relaxed) == 0)?;
        release(file);
        check(counts.owners.load(Ordering::Relaxed) == 1)?;
        check(counts.revokes.load(Ordering::Relaxed) == 1)?;
        check(counts.policies.load(Ordering::Relaxed) == 1)
    }

    #[test]
    fn revocation_does_not_destroy_retained_client_state() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = Authority::new(Arc::new(TestPolicy(counts.clone()), GFP_KERNEL)?)?;
        let file = authority.create_client_file(TestOwner(counts.clone()))?;
        authority.revoke();
        check(counts.revokes.load(Ordering::Relaxed) == 1)?;
        check(counts.owners.load(Ordering::Relaxed) == 0)?;
        release(file);
        check(counts.owners.load(Ordering::Relaxed) == 1)?;
        check(counts.policies.load(Ordering::Relaxed) == 0)?;
        drop(authority);
        check(counts.revokes.load(Ordering::Relaxed) == 1)?;
        check(counts.policies.load(Ordering::Relaxed) == 1)
    }
}
