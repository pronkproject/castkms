// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Callback-owned storage and retryable cleanup through a retained capture client file.

use super::*;
use crate::{
    drm::capture::{
        Authority,
        ClientDestination,
        ClientOwner,
        Policy, //
    },
    fs::File,
    sync::aref::ARef, //
};
use core::sync::atomic::AtomicBool;

#[derive(Default)]
struct Observed {
    registrations: AtomicU32,
    removals: AtomicU32,
    fail_remove: AtomicBool,
}

struct Owner {
    observed: Arc<Observed>,
    retained: Option<ARef<DmaBuf>>,
    last_id: u64,
}

// SAFETY: All callbacks and destruction belong to the built-in test module.
#[vtable]
unsafe impl ClientOwner for Owner {
    fn register_destination(&mut self, id: u64, destination: &Destination<'_>) -> Result {
        if id <= self.last_id {
            return Err(ESTALE);
        }
        if self.retained.is_some() {
            return Err(EBUSY);
        }
        if destination.num_planes() != 1 || destination.dimensions() != [16, 16] {
            return Err(EOPNOTSUPP);
        }
        let plane = destination.plane(0).ok_or(EINVAL)?;
        self.retained = Some(plane.buffer().into());
        self.last_id = id;
        self.observed.registrations.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn unregister_destination(&mut self, id: u64) -> Result {
        if self.observed.fail_remove.swap(false, Ordering::Relaxed) {
            return Err(EIO);
        }
        if self.last_id != id || self.retained.is_none() {
            return Err(ENOENT);
        }
        self.retained = None;
        self.observed.removals.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

struct Permission;

// SAFETY: The policy callback and its destructor belong to the built-in test module.
#[vtable]
unsafe impl Policy for Permission {
    fn revoke(&self) {}
}

fn with_client(test: impl FnOnce(&Authority<Permission>, &File, &Observed) -> Result) -> Result {
    let observed = Arc::new(Observed::default(), GFP_KERNEL)?;
    let authority = Authority::new(Arc::new(Permission, GFP_KERNEL)?)?;
    let file = authority.create_client_file(Owner {
        observed: observed.clone(),
        retained: None,
        last_id: 0,
    })?;
    let result = test(&authority, &file, &observed);
    // SAFETY: All callback-local handles have been dropped. Complete the owned file's
    // release before returning to the buffer fixture and releasing its parent device.
    unsafe { bindings::__fput_sync(ARef::into_raw(file).cast().as_ptr()) };
    result
}

#[kunit_tests(rust_drm_capture_destination_files)]
mod cases {
    use super::*;

    #[test]
    fn cleanup_retries_and_remains_available_after_revocation() -> Result {
        with_buffer(false, |buffer| {
            let destination = Destination::new(
                [16, 16],
                fourcc::XRGB8888,
                0,
                &[DestinationPlane::new(buffer, 64, 0)],
            )?;
            with_client(|authority, file, observed| {
                let mut registered = ClientDestination::register(file, 1, &destination)?;
                observed.fail_remove.store(true, Ordering::Relaxed);
                if registered.unregister() != Err(EIO) || registered.id() != Some(1) {
                    return Err(EINVAL);
                }
                drop(registered);
                if observed.removals.load(Ordering::Relaxed) != 1 {
                    return Err(EINVAL);
                }
                let mut next = ClientDestination::register(file, 2, &destination)?;
                authority.revoke();
                if !matches!(
                    ClientDestination::register(file, 3, &destination),
                    Err(EKEYREVOKED)
                ) {
                    return Err(EINVAL);
                }
                next.unregister()?;
                next.unregister()?;
                if next.id().is_some() || observed.removals.load(Ordering::Relaxed) != 2 {
                    return Err(EINVAL);
                }
                Ok(())
            })
        })
    }

    #[test]
    fn delayed_handle_cleanup_never_removes_a_new_name() -> Result {
        with_buffer(false, |buffer| {
            let destination = Destination::new(
                [16, 16],
                fourcc::XRGB8888,
                0,
                &[DestinationPlane::new(buffer, 64, 0)],
            )?;
            with_client(|_, file, observed| {
                let old = ClientDestination::register(file, 1, &destination)?;
                // SAFETY: The retained client is live; simulate removal by another holder
                // through the same native operation, without closing its file descriptor.
                crate::error::to_result(unsafe {
                    bindings::drm_capture_client_unregister_destination(file.as_ptr(), 1)
                })?;
                let next = ClientDestination::register(file, 2, &destination)?;
                drop(old);
                if observed.removals.load(Ordering::Relaxed) != 1 || next.id() != Some(2) {
                    return Err(EINVAL);
                }
                drop(next);
                if observed.removals.load(Ordering::Relaxed) != 2 {
                    return Err(EINVAL);
                }
                Ok(())
            })
        })
    }
}
