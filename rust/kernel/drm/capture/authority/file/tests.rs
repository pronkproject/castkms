// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::sync::Arc;
use core::sync::atomic::{
    AtomicU32,
    Ordering, //
};

#[derive(Default)]
struct Counts {
    revokes: AtomicU32,
    releases: AtomicU32,
    owner_releases: AtomicU32,
    revokes_seen_by_owner: AtomicU32,
    policy_releases_seen_by_owner: AtomicU32,
}

struct TestOwner(Arc<Counts>);

// SAFETY: The release trampoline and destructor belong to the kernel crate's LocalModule.
#[vtable]
unsafe impl ControlOwner for TestOwner {}

impl Drop for TestOwner {
    fn drop(&mut self) {
        self.0
            .revokes_seen_by_owner
            .store(self.0.revokes.load(Ordering::Relaxed), Ordering::Relaxed);
        self.0
            .policy_releases_seen_by_owner
            .store(self.0.releases.load(Ordering::Relaxed), Ordering::Relaxed);
        self.0.owner_releases.fetch_add(1, Ordering::Relaxed);
    }
}

struct RevokingOwner {
    authority: ARef<Authority<TestPolicy>>,
    _owner: TestOwner,
}

// SAFETY: The release trampoline and destructor belong to the kernel crate's LocalModule.
#[vtable]
unsafe impl ControlOwner for RevokingOwner {}

impl Drop for RevokingOwner {
    fn drop(&mut self) {
        self.authority.revoke();
    }
}

#[track_caller]
fn check(condition: bool) -> Result {
    if condition {
        Ok(())
    } else {
        let location = core::panic::Location::caller();
        pr_err!(
            "Control owner check failed at {}:{}\n",
            location.file(),
            location.line()
        );
        Err(EINVAL)
    }
}

struct TestPolicy(Arc<Counts>);

// SAFETY: Callback code and destruction belong to the kernel crate's LocalModule.
#[vtable]
unsafe impl Policy for TestPolicy {
    fn revoke(&self) {
        self.0.revokes.fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for TestPolicy {
    fn drop(&mut self) {
        self.0.releases.fetch_add(1, Ordering::Relaxed);
    }
}

fn authority(counts: &Arc<Counts>) -> Result<ARef<Authority<TestPolicy>>> {
    Authority::new(Arc::new(TestPolicy(counts.clone()), GFP_KERNEL)?)
}

fn release_file(file: ARef<File>) {
    // SAFETY: KUnit runs in a kernel thread; transfer one reference and synchronously finish
    // release so assertions can observe final fput, not the scheduling of deferred cleanup.
    unsafe { bindings::__fput_sync(ARef::into_raw(file).cast().as_ptr()) };
}

#[kunit_tests(rust_drm_capture_control_file)]
mod cases {
    use super::*;

    #[test]
    fn transferred_owner_lives_until_the_last_file_reference() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = authority(&counts)?;
        let file = authority.create_control_file_with_owner(TestOwner(counts.clone()))?;
        let duplicate = file.clone();
        release_file(file);
        check(counts.owner_releases.load(Ordering::Relaxed) == 0)?;
        check(!authority.is_revoked())?;
        drop(authority);
        release_file(duplicate);
        check(counts.owner_releases.load(Ordering::Relaxed) == 1)?;
        check(counts.revokes_seen_by_owner.load(Ordering::Relaxed) == 1)?;
        check(counts.policy_releases_seen_by_owner.load(Ordering::Relaxed) == 0)?;
        check(counts.releases.load(Ordering::Relaxed) == 1)?;
        Ok(())
    }

    #[test]
    fn revocation_does_not_drop_a_files_retained_owner() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = authority(&counts)?;
        let file = authority.create_control_file_with_owner(TestOwner(counts.clone()))?;
        authority.revoke();
        check(counts.owner_releases.load(Ordering::Relaxed) == 0)?;
        release_file(file);
        check(counts.owner_releases.load(Ordering::Relaxed) == 1)?;
        check(counts.revokes.load(Ordering::Relaxed) == 1)?;
        Ok(())
    }

    #[test]
    fn independent_control_files_release_their_own_owners() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = authority(&counts)?;
        let first = authority.create_control_file_with_owner(TestOwner(counts.clone()))?;
        let second = authority.create_control_file_with_owner(TestOwner(counts.clone()))?;
        release_file(first);
        check(counts.owner_releases.load(Ordering::Relaxed) == 1)?;
        check(authority.cleanup_done())?;
        release_file(second);
        check(counts.owner_releases.load(Ordering::Relaxed) == 2)?;
        check(counts.revokes.load(Ordering::Relaxed) == 1)?;
        Ok(())
    }

    #[test]
    fn owner_cleanup_may_reenter_revocation_without_authority_locks() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = authority(&counts)?;
        let owner = RevokingOwner {
            authority: authority.clone(),
            _owner: TestOwner(counts.clone()),
        };
        let file = authority.create_control_file_with_owner(owner)?;
        drop(authority);
        release_file(file);
        check(counts.owner_releases.load(Ordering::Relaxed) == 1)?;
        check(counts.revokes.load(Ordering::Relaxed) == 1)?;
        check(counts.releases.load(Ordering::Relaxed) == 1)?;
        Ok(())
    }

    #[test]
    fn an_already_revoked_authority_still_retains_the_transferred_owner() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = authority(&counts)?;
        authority.revoke();
        let file = authority.create_control_file_with_owner(TestOwner(counts.clone()))?;
        check(counts.owner_releases.load(Ordering::Relaxed) == 0)?;
        check(authority.cleanup_done())?;
        release_file(file);
        check(counts.owner_releases.load(Ordering::Relaxed) == 1)?;
        check(counts.revokes.load(Ordering::Relaxed) == 1)?;
        Ok(())
    }

    #[test]
    fn duplicate_file_references_share_revocation_lifetime() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = authority(&counts)?;
        let file = authority.create_control_file()?;
        let duplicate = file.clone();
        release_file(file);
        assert!(!authority.is_revoked());
        release_file(duplicate);
        assert!(authority.cleanup_done());
        assert_eq!(counts.revokes.load(Ordering::Relaxed), 1);
        assert_eq!(counts.releases.load(Ordering::Relaxed), 0);
        drop(authority);
        assert_eq!(counts.releases.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn file_retains_policy_after_last_kernel_authority_reference() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = authority(&counts)?;
        let file = authority.create_control_file()?;
        drop(authority);
        assert_eq!(counts.revokes.load(Ordering::Relaxed), 0);
        assert_eq!(counts.releases.load(Ordering::Relaxed), 0);
        release_file(file);
        assert_eq!(counts.revokes.load(Ordering::Relaxed), 1);
        assert_eq!(counts.releases.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn independent_files_each_have_revocation_power() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = authority(&counts)?;
        let first = authority.create_control_file()?;
        let second = authority.create_control_file()?;
        release_file(first);
        assert!(authority.cleanup_done());
        assert_eq!(counts.revokes.load(Ordering::Relaxed), 1);
        release_file(second);
        authority.revoke();
        assert_eq!(counts.revokes.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn file_creation_does_not_reopen_revoked_authority() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = authority(&counts)?;
        authority.revoke();
        let file = authority.create_control_file()?;
        assert!(authority.cleanup_done());
        assert!(matches!(authority.begin(), Err(EKEYREVOKED)));
        release_file(file);
        assert_eq!(counts.revokes.load(Ordering::Relaxed), 1);
        Ok(())
    }
}
