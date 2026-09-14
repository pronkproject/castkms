// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::{
    drm::capture::{
        Authority,
        ClientOwner,
        Policy, //
    },
    sync::Arc, //
};

struct TestPolicy;

// SAFETY: Policy callbacks belong to this kernel crate's LocalModule.
#[vtable]
unsafe impl Policy for TestPolicy {
    fn revoke(&self) {}
}

struct Owner;

// SAFETY: The owner's release trampoline and destruction belong to LocalModule.
#[vtable]
unsafe impl ClientOwner for Owner {}

fn release(file: ARef<File>) {
    // SAFETY: Transfer one owned file reference from the KUnit kernel thread and finish
    // release synchronously so the test observes close rather than deferred fput scheduling.
    unsafe { bindings::__fput_sync(ARef::into_raw(file).cast().as_ptr()) };
}

fn check(condition: bool) -> Result {
    if condition {
        Ok(())
    } else {
        Err(EINVAL)
    }
}

#[kunit_tests(rust_drm_capture_file_pair)]
mod cases {
    use super::*;

    #[test]
    fn retained_pair_preserves_endpoint_roles_and_close_lifetimes() -> Result {
        let authority = Authority::new(Arc::new(TestPolicy, GFP_KERNEL)?)?;
        let capture = authority.create_client_file(Owner)?;
        let control = authority.create_control_file()?;
        let pair = FilePair::from_files(&capture, &control)?;
        check(core::ptr::eq(pair.capture_file(), &*capture))?;
        check(core::ptr::eq(pair.control_file(), &*control))?;
        release(capture);
        release(control);
        check(!authority.is_revoked())?;
        let (capture, control) = pair.into_files();
        release(capture);
        check(!authority.is_revoked())?;
        release(control);
        check(authority.is_revoked())
    }

    #[test]
    fn mismatched_pair_rejection_does_not_close_callers_files() -> Result {
        let first = Authority::new(Arc::new(TestPolicy, GFP_KERNEL)?)?;
        let second = Authority::new(Arc::new(TestPolicy, GFP_KERNEL)?)?;
        let capture = first.create_client_file(Owner)?;
        let control = second.create_control_file()?;
        check(matches!(
            FilePair::from_files(&capture, &control),
            Err(EINVAL)
        ))?;
        check(matches!(
            FilePair::from_files(&control, &capture),
            Err(EINVAL)
        ))?;
        check(matches!(
            FilePair::from_files(&capture, &capture),
            Err(EINVAL)
        ))?;
        check(!first.is_revoked())?;
        check(!second.is_revoked())?;
        release(capture);
        check(!first.is_revoked())?;
        release(control);
        check(second.is_revoked())
    }
}
