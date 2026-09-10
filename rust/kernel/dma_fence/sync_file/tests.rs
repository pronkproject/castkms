// SPDX-License-Identifier: GPL-2.0

use super::*;
use crate::dma_fence::{
    testing::ManualFence,
    Status, //
};

fn release_file(file: ARef<File>) {
    // SAFETY: KUnit executes in a kernel thread. Transfer one owned reference and finish
    // release synchronously so assertions observe cleanup rather than deferred fput scheduling.
    unsafe { bindings::__fput_sync(ARef::into_raw(file).cast().as_ptr()) };
}

/// Borrow the record retained by a known sync file.
///
/// # Safety
///
/// The file must have been returned by Fence::create_sync_file.
unsafe fn retained_fence(file: &File) -> &Fence {
    // SAFETY: The caller identifies a sync file; the file retains its immutable native fence.
    let raw = unsafe { (*file.as_ptr()).private_data.cast::<bindings::sync_file>() };
    // SAFETY: The returned reference is bounded by the file's lifetime.
    unsafe { Fence::from_raw((*raw).fence) }
}

/// Observe a known sync file without registering a poll waiter.
///
/// # Safety
///
/// The file must have been returned by Fence::create_sync_file.
unsafe fn poll(file: &File) -> u32 {
    // SAFETY: A retained sync file has a poll callback. Null poll-table storage requests an
    // observation without registering a waiter; native fence-callback ownership stays in file.
    unsafe { (*(*file.as_ptr()).f_op).poll.unwrap()(file.as_ptr(), core::ptr::null_mut()) }
}

#[kunit_tests(rust_dma_fence_sync_file)]
mod cases {
    use super::*;

    #[test]
    fn extracted_fence_survives_closing_its_file() -> Result {
        let mut owner = ManualFence::new()?;
        let original = owner.fence();
        let file = original.create_sync_file()?;
        let extracted = Fence::from_sync_file(&file)?;
        assert_eq!(extracted.as_raw(), original.as_raw());
        release_file(file);
        drop(original);
        assert_eq!(extracted.status(), Status::Pending);
        owner.complete(Err(EIO))?;
        drop(owner);
        assert_eq!(extracted.status(), Status::Complete(Err(EIO)));
        Ok(())
    }

    #[test]
    fn completed_error_survives_file_extraction() -> Result {
        let mut owner = ManualFence::new()?;
        owner.complete(Err(EIO))?;
        let file = owner.fence().create_sync_file()?;
        let fence = Fence::from_sync_file(&file)?;
        release_file(file);
        drop(owner);
        assert_eq!(fence.status(), Status::Complete(Err(EIO)));
        Ok(())
    }

    #[test]
    fn unrelated_file_is_rejected_before_private_data_access() -> Result {
        const OPS: &bindings::file_operations = &pin_init::zeroed();
        // SAFETY: The immutable empty operations table has static lifetime and no module
        // callbacks. The unpublished anonymous file needs no private data or release callback.
        let file = crate::error::from_err_ptr(unsafe {
            bindings::anon_inode_getfile(
                c"rust-not-sync".as_char_ptr(),
                OPS,
                core::ptr::null_mut(),
                0,
            )
        })?;
        // SAFETY: Creation transfers an initialized unpublished file reference; no fdget_pos
        // operation exists on it. The transparent Rust file type adopts that exact reference.
        let file = unsafe { ARef::<File>::from_raw(NonNull::new_unchecked(file.cast())) };
        assert!(matches!(Fence::from_sync_file(&file), Err(EINVAL)));
        release_file(file);
        Ok(())
    }

    #[test]
    fn pending_file_observes_later_producer_failure() -> Result {
        let mut owner = ManualFence::new()?;
        let fence = owner.fence();
        let file = fence.create_sync_file()?;
        // SAFETY: The file was created by the fence's native sync-file constructor.
        let retained = unsafe { retained_fence(&file) };
        assert_eq!(retained.as_raw(), fence.as_raw());
        assert_eq!(retained.status(), Status::Pending);
        // SAFETY: The retained file is a native sync file.
        assert_eq!(unsafe { poll(&file) }, 0);
        owner.complete(Err(EIO))?;
        drop(fence);
        drop(owner);
        // SAFETY: File ownership independently retains the original native fence.
        let (status, readiness) = unsafe { (retained_fence(&file).status(), poll(&file)) };
        assert_eq!(status, Status::Complete(Err(EIO)));
        assert_ne!(readiness & bindings::POLLIN, 0);
        release_file(file);
        Ok(())
    }

    #[test]
    fn completed_files_preserve_success_and_error() -> Result {
        for result in [Ok(()), Err(EIO)] {
            let mut owner = ManualFence::new()?;
            owner.complete(result)?;
            let file = owner.fence().create_sync_file()?;
            drop(owner);
            // SAFETY: The newly created file retains the completed native record.
            let (status, readiness) = unsafe { (retained_fence(&file).status(), poll(&file)) };
            assert_eq!(status, Status::Complete(result));
            assert_ne!(readiness & bindings::POLLIN, 0);
            release_file(file);
        }
        Ok(())
    }

    #[test]
    fn dropping_polled_files_does_not_complete_pending_work() -> Result {
        let mut owner = ManualFence::new()?;
        let first = owner.fence().create_sync_file()?;
        let second = owner.fence().create_sync_file()?;
        let duplicate = first.clone();
        assert_ne!(first.as_ptr(), second.as_ptr());
        assert_eq!(first.as_ptr(), duplicate.as_ptr());
        // SAFETY: Both independent files came from the native sync-file constructor.
        let (first_ready, second_ready) = unsafe { (poll(&first), poll(&second)) };
        assert_eq!(first_ready, 0);
        assert_eq!(second_ready, 0);
        release_file(first);
        release_file(duplicate);
        release_file(second);
        assert_eq!(owner.fence().status(), Status::Pending);
        owner.complete(Ok(()))?;
        assert_eq!(owner.fence().status(), Status::Complete(Ok(())));
        Ok(())
    }
}
