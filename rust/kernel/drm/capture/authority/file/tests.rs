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
