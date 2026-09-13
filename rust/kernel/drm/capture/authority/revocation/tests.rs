// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::{
    drm::capture::{
        Status,
        Stream, //
    },
    prelude::*,
    sync::Arc, //
};
use core::sync::atomic::{
    AtomicU32,
    Ordering, //
};

#[derive(Default)]
struct Counts {
    revokes: AtomicU32,
    releases: AtomicU32,
}

struct TestPolicy<const ID: u8>(Arc<Counts>);

// SAFETY: All monomorphized callbacks and destruction belong to the local kernel module.
#[vtable]
unsafe impl<const ID: u8> Policy for TestPolicy<ID> {
    fn revoke(&self) {
        self.0.revokes.fetch_add(1, Ordering::Relaxed);
    }
}

impl<const ID: u8> Drop for TestPolicy<ID> {
    fn drop(&mut self) {
        self.0.releases.fetch_add(1, Ordering::Relaxed);
    }
}

fn authority<const ID: u8>(counts: &Arc<Counts>) -> Result<ARef<Authority<TestPolicy<ID>>>> {
    Authority::new(Arc::new(TestPolicy(counts.clone()), GFP_KERNEL)?)
}

#[track_caller]
fn check(condition: bool) -> Result {
    if condition {
        Ok(())
    } else {
        let location = core::panic::Location::caller();
        pr_err!(
            "Revocation check failed at {}:{}\n",
            location.file(),
            location.line()
        );
        Err(EINVAL)
    }
}

#[kunit_tests(rust_drm_capture_revocation)]
mod cases {
    use super::*;

    #[test]
    fn erased_reference_retains_original_policy_until_final_release() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = authority::<1>(&counts)?;
        let revocation = authority.revocation();
        let duplicate = revocation.clone();
        drop(authority);
        drop(revocation);
        check(counts.revokes.load(Ordering::Relaxed) == 0)?;
        check(counts.releases.load(Ordering::Relaxed) == 0)?;
        duplicate.revoke();
        check(duplicate.is_revoked() && duplicate.cleanup_done())?;
        check(counts.revokes.load(Ordering::Relaxed) == 1)?;
        check(counts.releases.load(Ordering::Relaxed) == 0)?;
        drop(duplicate);
        check(counts.releases.load(Ordering::Relaxed) == 1)?;
        check(counts.revokes.load(Ordering::Relaxed) == 1)?;
        Ok(())
    }

    #[test]
    fn releasing_a_view_does_not_revoke_a_surviving_authority() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = authority::<1>(&counts)?;
        drop(authority.revocation());
        check(!authority.is_revoked())?;
        check(!authority.cleanup_done())?;
        drop(authority);
        check(counts.revokes.load(Ordering::Relaxed) == 1)?;
        check(counts.releases.load(Ordering::Relaxed) == 1)?;
        Ok(())
    }

    #[test]
    fn erased_revocation_ends_the_same_registered_streams() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = authority::<1>(&counts)?;
        let stream = Stream::new(1, 8)?;
        authority.begin()?.add_stream(&stream)?;
        let request = stream.queue()?;
        authority.revocation().revoke();
        check(authority.is_revoked() && authority.cleanup_done())?;
        check(matches!(authority.begin(), Err(EKEYREVOKED)))?;
        check(request.status()? == Status::Complete(Err(EKEYREVOKED)))?;
        Ok(())
    }

    #[test]
    fn heterogeneous_providers_keep_independent_callbacks() -> Result {
        let first_counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let second_counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let first = authority::<1>(&first_counts)?;
        let second = authority::<2>(&second_counts)?;
        let references = [first.revocation(), second.revocation()];
        references[0].revoke();
        check(first.cleanup_done())?;
        check(!second.is_revoked())?;
        check(first_counts.revokes.load(Ordering::Relaxed) == 1)?;
        check(second_counts.revokes.load(Ordering::Relaxed) == 0)?;
        references[1].revoke();
        check(second.cleanup_done())?;
        check(second_counts.revokes.load(Ordering::Relaxed) == 1)?;
        Ok(())
    }
}
