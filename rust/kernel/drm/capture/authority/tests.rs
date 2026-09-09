// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::drm::capture::Status;
use core::sync::atomic::{
    AtomicBool,
    AtomicU32,
    Ordering, //
};

#[derive(Default)]
struct Counts {
    revokes: AtomicU32,
    releases: AtomicU32,
    approvals: AtomicU32,
}

struct TestPolicy {
    counts: Arc<Counts>,
    allow: AtomicBool,
}

impl Drop for TestPolicy {
    fn drop(&mut self) {
        self.counts.releases.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: The callbacks and policy destruction are built into the kernel crate's LocalModule.
#[vtable]
unsafe impl Policy for TestPolicy {
    fn revoke(&self) {
        self.counts.revokes.fetch_add(1, Ordering::Relaxed);
    }

    fn authorize_capture(&self, _: &Stream) -> Result {
        self.counts.approvals.fetch_add(1, Ordering::Relaxed);
        if self.allow.load(Ordering::Relaxed) {
            Ok(())
        } else {
            Err(EACCES)
        }
    }
}

fn policy(counts: &Arc<Counts>, allow: bool) -> Result<Arc<TestPolicy>> {
    Ok(Arc::new(
        TestPolicy {
            counts: counts.clone(),
            allow: AtomicBool::new(allow),
        },
        GFP_KERNEL,
    )?)
}

struct NoClaims;

// SAFETY: The callback and policy destruction are built into the kernel crate's LocalModule.
#[vtable]
unsafe impl Policy for NoClaims {
    fn revoke(&self) {}
}

#[kunit_tests(rust_drm_capture_authority)]
mod cases {
    use super::*;

    #[test]
    fn final_reference_revokes_and_releases_provider_once() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = Authority::new(policy(&counts, true)?)?;
        let retained = authority.clone();
        drop(authority);
        assert_eq!(counts.revokes.load(Ordering::Relaxed), 0);
        assert_eq!(counts.releases.load(Ordering::Relaxed), 0);
        drop(retained);
        assert_eq!(counts.revokes.load(Ordering::Relaxed), 1);
        assert_eq!(counts.releases.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn explicit_revoke_is_once_and_external_policy_ownership_survives() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let policy = policy(&counts, true)?;
        let authority = Authority::new(policy.clone())?;
        assert!(!authority.is_revoked());
        assert!(!authority.cleanup_done());
        authority.revoke();
        authority.revoke();
        assert!(authority.is_revoked());
        assert!(authority.cleanup_done());
        assert!(matches!(authority.begin(), Err(EKEYREVOKED)));
        drop(authority);
        assert_eq!(counts.revokes.load(Ordering::Relaxed), 1);
        assert_eq!(counts.releases.load(Ordering::Relaxed), 0);
        drop(policy);
        assert_eq!(counts.releases.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn guarded_registration_rejects_duplicate_streams() -> Result {
        let authority = Authority::new(Arc::new(NoClaims, GFP_KERNEL)?)?;
        let stream = Stream::new(1, 16)?;
        let admission = authority.begin()?;
        admission.add_stream(&stream)?;
        assert_eq!(admission.add_stream(&stream), Err(EEXIST));
        drop(admission);
        authority.revoke();
        assert!(matches!(stream.queue(), Err(EKEYREVOKED)));
        Ok(())
    }

    #[test]
    fn missing_policy_callback_disables_claims() -> Result {
        let authority = Authority::new(Arc::new(NoClaims, GFP_KERNEL)?)?;
        let stream = Stream::new(1, 16)?;
        authority.begin()?.add_stream(&stream)?;
        let request = stream.queue()?;
        assert!(matches!(authority.claim(&stream), Err(EOPNOTSUPP)));
        assert_eq!(request.status()?, Status::Pending);
        Ok(())
    }

    #[test]
    fn membership_and_policy_denial_leave_requests_unclaimed() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let policy = policy(&counts, false)?;
        let authority = Authority::new(policy.clone())?;
        let stream = Stream::new(1, 16)?;
        let request = stream.queue()?;
        assert!(matches!(authority.claim(&stream), Err(ENOENT)));
        assert_eq!(counts.approvals.load(Ordering::Relaxed), 0);
        authority.begin()?.add_stream(&stream)?;
        assert!(matches!(authority.claim(&stream), Err(EACCES)));
        assert_eq!(request.status()?, Status::Pending);
        policy.allow.store(true, Ordering::Relaxed);
        let mut job = authority.claim(&stream)?;
        job.data_mut().fill(0x31);
        job.complete(Ok(()));
        let mut output = [0; 16];
        request.copy_result(&mut output)?;
        assert_eq!(output, [0x31; 16]);
        assert_eq!(counts.approvals.load(Ordering::Relaxed), 2);
        Ok(())
    }

    #[test]
    fn revoke_preserves_active_job_but_rejects_its_delivery() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = Authority::new(policy(&counts, true)?)?;
        let stream = Stream::new(1, 16)?;
        authority.begin()?.add_stream(&stream)?;
        let request = stream.queue()?;
        let mut job = authority.claim(&stream)?;
        authority.revoke();
        assert!(matches!(authority.claim(&stream), Err(EKEYREVOKED)));
        assert_eq!(counts.approvals.load(Ordering::Relaxed), 1);
        drop(authority);
        assert_eq!(counts.revokes.load(Ordering::Relaxed), 1);
        assert_eq!(counts.releases.load(Ordering::Relaxed), 1);
        job.data_mut().fill(0xff);
        job.complete(Ok(()));
        assert_eq!(request.status()?, Status::Complete(Err(EKEYREVOKED)));
        Ok(())
    }

    #[test]
    fn removal_stops_one_stream_without_revoking_its_sibling() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let authority = Authority::new(policy(&counts, true)?)?;
        let first = Stream::new(1, 16)?;
        let second = Stream::new(1, 16)?;
        {
            let admission = authority.begin()?;
            admission.add_stream(&first)?;
            admission.add_stream(&second)?;
        }
        assert!(authority.remove_stream(&first));
        assert!(!authority.remove_stream(&first));
        assert!(matches!(first.queue(), Err(EKEYREVOKED)));
        assert!(!authority.is_revoked());
        let request = second.queue()?;
        authority.claim(&second)?.complete(Ok(()));
        assert_eq!(request.status()?, Status::Complete(Ok(())));
        Ok(())
    }
}
