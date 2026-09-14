// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::{
    drm::capture::{
        Authority,
        ClientOwner,
        Policy,
        Readiness, //
    },
    fs::File,
    sync::{
        aref::ARef,
        Arc, //
    },
    types::ScopeGuard, //
};

struct TestPolicy;

// SAFETY: Policy callbacks and destruction belong to LocalModule.
#[vtable]
unsafe impl Policy for TestPolicy {
    fn revoke(&self) {}
}

struct Owner {
    opened: bool,
    cancelled: bool,
    readiness: ARef<Readiness>,
}

// SAFETY: Owner callbacks and destruction belong to LocalModule.
#[vtable]
unsafe impl ClientOwner for Owner {
    fn open_stream(&mut self, id: u64, offer: u64, capacity: u32) -> Result {
        if id != 7 || offer != 9 || capacity != 1 || self.opened {
            return Err(EINVAL);
        }
        self.opened = true;
        Ok(())
    }

    fn close_stream(&mut self, id: u64) -> Result {
        if id != 7 || !self.opened {
            return Err(ENOENT);
        }
        self.opened = false;
        Ok(())
    }

    fn readiness(&self) -> Option<&Readiness> {
        Some(&self.readiness)
    }

    fn cancel(&mut self, stream: u64, use_id: u64) -> Result {
        if stream != 7 || use_id != 11 || !self.opened {
            return Err(ENOENT);
        }
        if self.cancelled {
            return Err(EALREADY);
        }
        self.cancelled = true;
        Ok(())
    }
}

fn with_client(run: impl FnOnce(&Authority<TestPolicy>, &File, ClientStream) -> Result) -> Result {
    let authority = Authority::new(Arc::new(TestPolicy, GFP_KERNEL)?)?;
    let file = authority.create_client_file(Owner {
        opened: false,
        cancelled: false,
        readiness: Readiness::new()?,
    })?;
    let file = ScopeGuard::new_with_data(file, |file| {
        // SAFETY: The callback has returned, dropping its stream before this final owned
        // reference is released synchronously from the KUnit task outside driver locks.
        unsafe { bindings::__fput_sync(ARef::into_raw(file).cast().as_ptr()) };
    });
    let stream = ClientStream::open(&file, 7, 9, 1)?;
    run(&authority, &file, stream)
}

#[kunit_tests(rust_drm_capture_client_cancel)]
mod cases {
    use super::*;

    #[test]
    fn cancellation_preserves_names_errors_and_pending_readiness() -> Result {
        with_client(|_, file, stream| {
            let readiness = Readiness::for_client(file)?;
            assert_eq!(stream.cancel(0), Err(EINVAL));
            assert_eq!(stream.cancel(12), Err(ENOENT));
            stream.cancel(11)?;
            assert_eq!(stream.cancel(11), Err(EALREADY));
            assert!(!readiness.has_results());
            Ok(())
        })
    }

    #[test]
    fn cancellation_survives_revocation_but_not_acknowledged_closure() -> Result {
        with_client(|authority, _, mut stream| {
            authority.revoke();
            stream.cancel(11)?;
            stream.close()?;
            assert_eq!(stream.cancel(11), Err(ENOENT));
            Ok(())
        })
    }
}
