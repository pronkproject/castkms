// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::sync::Arc;
use core::cell::Cell;
use core::sync::atomic::{
    AtomicU32,
    Ordering, //
};

#[derive(Default)]
struct Counts {
    revokes: AtomicU32,
    owners: AtomicU32,
    policies: AtomicU32,
}

struct TestOwner(Arc<Counts>);

struct DescribingOwner(Cell<u64>);

// SAFETY: The callback and owner destruction belong to LocalModule.
#[vtable]
unsafe impl ClientOwner for DescribingOwner {
    fn describe(&mut self) -> Result<Description> {
        let id = self.0.get().checked_add(1).ok_or(EOVERFLOW)?;
        self.0.set(id);
        Description::new(id, [64, 32], crate::drm::fourcc::XRGB8888, 0, 8)
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
