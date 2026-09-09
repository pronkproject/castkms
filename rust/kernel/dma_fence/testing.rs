// SPDX-License-Identifier: GPL-2.0

//! Manually completed fences for deterministic kernel tests only.
//!
//! These fixtures must not be used to represent future userspace work in a production
//! dependency graph. Dropping the signal owner completes an abandoned test fence with EIO.

use super::*;

unsafe extern "C" fn name(_: *mut bindings::dma_fence) -> *const crate::ffi::c_char {
    c"rust-fence-test".as_char_ptr()
}

static OPS: bindings::dma_fence_ops = bindings::dma_fence_ops {
    get_driver_name: Some(name),
    get_timeline_name: Some(name),
    enable_signaling: None,
    signaled: None,
    wait: None,
    release: None,
    set_deadline: None,
};

unsafe extern "C" fn fail_on_enable(raw: *mut bindings::dma_fence) -> bool {
    // SAFETY: Native fence wait calls this callback under the fence lock before signaling.
    // Returning false asks the native core to publish completion with the supplied error.
    unsafe { (*raw).error = EIO.to_errno() };
    false
}

static FAIL_ON_WAIT_OPS: bindings::dma_fence_ops = bindings::dma_fence_ops {
    enable_signaling: Some(fail_on_enable),
    ..OPS
};

fn allocate(ops: &'static bindings::dma_fence_ops) -> Result<ARef<Fence>> {
    let storage = KBox::new(Opaque::<bindings::dma_fence>::zeroed(), GFP_KERNEL)?;
    let raw = storage.get();
    // SAFETY: Stable uninitialized storage, permanent built-in callbacks and a native inline
    // lock. The default native release frees the kmalloc allocation through RCU; no Rust
    // payload requires destruction.
    unsafe {
        bindings::dma_fence_init(
            raw,
            ops,
            core::ptr::null_mut(),
            bindings::dma_fence_context_alloc(1),
            1,
        );
    }
    let raw = KBox::into_raw(storage).cast::<Fence>();
    // SAFETY: Native initialization created one reference in the transferred allocation.
    Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(raw)) })
}

/// A pending test dependency that fails when native waiting enables software signaling.
/// No manual signal owner exists; status inspection alone does not trigger completion.
pub fn fail_when_waited() -> Result<ARef<Fence>> {
    allocate(&FAIL_ON_WAIT_OPS)
}

/// The single signal owner of a test fence. Readers only receive read-only references.
pub struct ManualFence {
    fence: ARef<Fence>,
    completed: bool,
}

impl ManualFence {
    /// Allocate a private fence with its native inline spinlock.
    pub fn new() -> Result<Self> {
        Ok(Self {
            fence: allocate(&OPS)?,
            completed: false,
        })
    }

    /// Retain a read-only completion record.
    pub fn fence(&self) -> ARef<Fence> {
        self.fence.clone()
    }

    /// Complete the private fence exactly once; repeated attempts fail without mutation.
    pub fn complete(&mut self, result: Result) -> Result {
        if self.completed {
            return Err(EALREADY);
        }
        // SAFETY: This is the only signal owner, and no custom signaled callback exists.
        // Native readers do not inspect error until signal publishes it under the fence lock.
        if let Err(error) = result {
            unsafe { (*self.fence.as_raw()).error = error.to_errno() };
        }
        // SAFETY: The live fence has no other signal path; its native lock serializes readers.
        unsafe { bindings::dma_fence_signal(self.fence.as_raw()) };
        self.completed = true;
        Ok(())
    }
}

impl Drop for ManualFence {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self.complete(Err(EIO));
        }
    }
}

#[kunit_tests(rust_dma_fence)]
mod cases {
    use super::*;

    #[test]
    fn pending_then_success_remains_success_after_owner_drop() -> Result {
        let mut owner = ManualFence::new()?;
        let fence = owner.fence();
        assert_eq!(fence.status(), Status::Pending);
        owner.complete(Ok(()))?;
        drop(owner);
        assert_eq!(fence.status(), Status::Complete(Ok(())));
        Ok(())
    }

    #[test]
    fn failure_is_retained_before_and_after_collection() -> Result {
        let mut owner = ManualFence::new()?;
        let before = owner.fence();
        owner.complete(Err(EIO))?;
        let after = owner.fence();
        drop(owner);
        assert_eq!(before.status(), Status::Complete(Err(EIO)));
        assert_eq!(after.status(), Status::Complete(Err(EIO)));
        Ok(())
    }

    #[test]
    fn abandoned_signal_owner_completes_with_error() -> Result {
        let owner = ManualFence::new()?;
        let fence = owner.fence();
        drop(owner);
        assert_eq!(fence.status(), Status::Complete(Err(EIO)));
        Ok(())
    }

    #[test]
    fn completion_cannot_be_rewritten() -> Result {
        let mut owner = ManualFence::new()?;
        owner.complete(Err(EIO))?;
        assert_eq!(owner.complete(Ok(())), Err(EALREADY));
        assert_eq!(owner.fence().status(), Status::Complete(Err(EIO)));
        Ok(())
    }
}
