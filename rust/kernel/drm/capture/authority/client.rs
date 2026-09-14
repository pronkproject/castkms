// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Capture-client file lifetime without ownership of authority revocation.

use super::{
    Authority,
    Policy, //
};
use crate::{
    dma_fence::Fence,
    drm::capture::{
        Completion,
        Description,
        Destination,
        Readiness, //
    },
    error::from_err_ptr,
    fs::File,
    prelude::*,
    sync::aref::ARef, //
};
use core::{
    ffi::c_void,
    marker::PhantomData,
    ptr::NonNull, //
};

/// Client state retained until final release of its anonymous file.
///
/// The file itself does not request revocation. A client owner should release only its own
/// resources, without taking responsibility for revoking the authority shared by siblings.
///
/// # Safety
///
/// `OwnerModule` must retain this type's callback trampolines, destructor and dependencies
/// throughout the transferred lifetime. The vtable macro selects the local module.
#[vtable]
pub unsafe trait ClientOwner: Send + 'static {
    /// Admit an output attempt using this client's registered destination.
    ///
    /// Retain the exact destination and an owned reference to any borrowed reuse fence
    /// before returning success. Rejecting an attempt must not consume its name or start
    /// destination access. Native dispatch requires readiness, dequeue and stream cleanup
    /// support too. Destination waits must not retain compositor sources.
    fn queue_output(
        &mut self,
        _stream: u64,
        _use_id: u64,
        _destination: u64,
        _reuse: Option<&Fence>,
    ) -> Result {
        Err(EOPNOTSUPP)
    }

    /// Publish one terminal result without acknowledging it on publication failure.
    ///
    /// The closure is synchronous and called at most once. Return its result unchanged;
    /// success must acknowledge exactly that record, while failure retains it and its
    /// accounting credit for retry. Return EAGAIN when no result is available. All destination
    /// access for a published attempt must have ended, including on terminal failure.
    fn dequeue(&mut self, _stream: u64, _publish: impl FnOnce(Completion) -> Result) -> Result {
        Err(EOPNOTSUPP)
    }

    /// Supply a notification to retain independently of this owner's mutable state.
    ///
    /// Called once before the client file is published, never during polling. The file
    /// takes its own native reference before this borrow ends. Publish availability only
    /// after updating the result queue, and clear it when no terminal records remain.
    fn readiness(&self) -> Option<&Readiness> {
        None
    }

    /// Describe a currently authorized offer without allocating images or reading sources.
    ///
    /// Native dispatch serializes calls and retains this owner through their completion.
    /// Check current provider permission even for an unchanged offer. Do not reenter the
    /// same client file; call outside DRM and authority locks. Omitting the method leaves
    /// description queries unsupported.
    fn describe(&mut self) -> Result<Description> {
        Err(EOPNOTSUPP)
    }

    /// Open the named offer under a new, increasing nonzero stream ID.
    ///
    /// Serialize provider registration with revocation as well as checking current
    /// permission and capacity. The file mutex excludes other client callbacks, not
    /// authority revocation. Failure must leave the name retryable and retain no new
    /// stream. Opening through a file requires both open and close implementations.
    fn open_stream(&mut self, _id: u64, _offer: u64, _capacity: u32) -> Result {
        Err(EOPNOTSUPP)
    }

    /// Close one named stream without requiring permission to capture more pixels.
    ///
    /// Removed names must never be reused. Preserve native completion ownership
    /// independently of result delivery. Return ENOENT when the entry is absent;
    /// other errors must leave cleanup retryable. Do not reenter the client file.
    /// Successful closure requires every destination write admitted by that stream to
    /// have ended; shared rendering and downstream storage users remain independent.
    fn close_stream(&mut self, _id: u64) -> Result {
        Err(EOPNOTSUPP)
    }

    /// Register borrowed destination storage under a new, increasing client-local name.
    ///
    /// Check current capture permission, complete image layout and resource limits.
    /// Retained buffers need owned references; the description itself is borrowed only
    /// for this callback. Registration grants no future pixel authority or reuse exclusion.
    /// Failure must not consume the name. Native dispatch requires the cleanup callback too.
    fn register_destination(&mut self, _id: u64, _destination: &Destination<'_>) -> Result {
        Err(EOPNOTSUPP)
    }

    /// Remove a destination name even after revocation, preserving accepted use ownership.
    ///
    /// Removed names must never be reused. Return ENOENT for an absent entry; other errors
    /// must leave cleanup retryable. No operation may reenter the same client file.
    fn unregister_destination(&mut self, _id: u64) -> Result {
        Err(EOPNOTSUPP)
    }
}

struct Callbacks<O>(PhantomData<O>);

impl<O: ClientOwner> Callbacks<O> {
    const OPS: bindings::drm_capture_client_owner_ops = bindings::drm_capture_client_owner_ops {
        owner: crate::module::this_module::<O::OwnerModule>().as_ptr(),
        release: Some(Self::release),
        get_readiness: if O::HAS_READINESS {
            Some(Self::get_readiness)
        } else {
            None
        },
        describe: if O::HAS_DESCRIBE {
            Some(Self::describe)
        } else {
            None
        },
        open_stream: if O::HAS_OPEN_STREAM {
            Some(Self::open_stream)
        } else {
            None
        },
        close_stream: if O::HAS_CLOSE_STREAM {
            Some(Self::close_stream)
        } else {
            None
        },
        register_destination: if O::HAS_REGISTER_DESTINATION {
            Some(Self::register_destination)
        } else {
            None
        },
        unregister_destination: if O::HAS_UNREGISTER_DESTINATION {
            Some(Self::unregister_destination)
        } else {
            None
        },
        dequeue: if O::HAS_DEQUEUE {
            Some(Self::dequeue)
        } else {
            None
        },
        queue_output: if O::HAS_QUEUE_OUTPUT {
            Some(Self::queue_output)
        } else {
            None
        },
        cancel: None,
    };

    unsafe extern "C" fn queue_output(
        data: *mut c_void,
        stream: u64,
        use_id: u64,
        destination: u64,
        reuse: *mut bindings::dma_fence,
    ) -> i32 {
        // SAFETY: Native dispatch retains the file and exclusively borrows its initialized
        // KBox<O> under the client mutex for this callback.
        let owner = unsafe { &mut *data.cast::<O>() };
        let reuse = if reuse.is_null() {
            None
        } else {
            // SAFETY: The caller retains the optional fence through synchronous dispatch.
            Some(unsafe { Fence::from_raw(reuse) })
        };
        owner
            .queue_output(stream, use_id, destination, reuse)
            .map_or_else(|error| error.to_errno(), |_| 0)
    }

    unsafe extern "C" fn dequeue(
        data: *mut c_void,
        stream: u64,
        sink: *const bindings::drm_capture_completion_sink,
    ) -> i32 {
        // SAFETY: Native dispatch retains the client and exclusively borrows its initialized
        // KBox<O> under the operation mutex. The checked sink lives through this call.
        let owner = unsafe { &mut *data.cast::<O>() };
        // SAFETY: The native dispatcher supplies a live, immutable callback-local sink.
        let sink = unsafe { &*sink };
        let Some(publish) = sink.publish else {
            return EINVAL.to_errno();
        };
        let result = owner.dequeue(stream, |completion| {
            // SAFETY: The native publisher borrows the complete local metadata and its own
            // context synchronously. Neither borrow escapes this closure.
            crate::error::to_result(unsafe { publish(sink.data, &completion.raw()) })
        });
        result.map_or_else(|error| error.to_errno(), |_| 0)
    }

    unsafe extern "C" fn get_readiness(data: *mut c_void) -> *mut bindings::drm_capture_readiness {
        // SAFETY: Creation exclusively owns the initialized KBox<O> before file publication.
        // No other file callback can run until this call returns.
        let owner = unsafe { &*data.cast::<O>() };
        match owner.readiness() {
            Some(readiness) => {
                let retained: ARef<Readiness> = readiness.into();
                ARef::into_raw(retained).cast().as_ptr()
            }
            None => core::ptr::null_mut(),
        }
    }

    unsafe extern "C" fn release(data: *mut c_void) {
        // SAFETY: Creation transfers one initialized KBox<O> on success. Native final file
        // release returns it exactly once while retaining O's callback module.
        drop(unsafe { KBox::from_raw(data.cast::<O>()) });
    }

    unsafe extern "C" fn describe(
        data: *mut c_void,
        output: *mut bindings::drm_capture_description,
    ) -> i32 {
        // SAFETY: Creation binds the callback to one owned KBox<O>. Native dispatch holds
        // the client's mutex, excluding every other callback, and retains the file so its
        // final release cannot destroy O. No reference to O escapes this call.
        let owner = unsafe { &mut *data.cast::<O>() };
        match owner.describe() {
            Ok(description) => {
                // SAFETY: Native dispatch supplies writable callback-local output storage.
                unsafe { output.write(description.raw()) };
                0
            }
            Err(error) => error.to_errno(),
        }
    }

    unsafe extern "C" fn open_stream(data: *mut c_void, id: u64, offer: u64, capacity: u32) -> i32 {
        // SAFETY: Native dispatch retains the file and holds its client mutex, exclusively
        // borrowing the KBox<O> installed at creation. No reference escapes this callback.
        let owner = unsafe { &mut *data.cast::<O>() };
        match owner.open_stream(id, offer, capacity) {
            Ok(()) => 0,
            Err(error) => error.to_errno(),
        }
    }

    unsafe extern "C" fn close_stream(data: *mut c_void, id: u64) -> i32 {
        // SAFETY: The same file lifetime and client mutex exclude every other callback
        // and final destruction while this exclusive borrow of the transferred O exists.
        let owner = unsafe { &mut *data.cast::<O>() };
        match owner.close_stream(id) {
            Ok(()) => 0,
            Err(error) => error.to_errno(),
        }
    }

    unsafe extern "C" fn register_destination(
        data: *mut c_void,
        id: u64,
        raw: *const bindings::drm_capture_destination,
    ) -> i32 {
        // SAFETY: Native dispatch supplies stable metadata and retains all active buffers
        // throughout the callback. The description is not retained beyond this call.
        let destination = match unsafe { Destination::from_raw(raw) } {
            Ok(destination) => destination,
            Err(error) => return error.to_errno(),
        };
        // SAFETY: Native dispatch retains the file and serializes every callback with its
        // client mutex. The creation-time KBox<O> remains exclusively borrowed here.
        let owner = unsafe { &mut *data.cast::<O>() };
        match owner.register_destination(id, &destination) {
            Ok(()) => 0,
            Err(error) => error.to_errno(),
        }
    }

    unsafe extern "C" fn unregister_destination(data: *mut c_void, id: u64) -> i32 {
        // SAFETY: File retention and the client mutex exclude concurrent access and final
        // destruction of the transferred KBox<O> throughout this exclusive borrow.
        let owner = unsafe { &mut *data.cast::<O>() };
        match owner.unregister_destination(id) {
            Ok(()) => 0,
            Err(error) => error.to_errno(),
        }
    }
}

impl<P: Policy> Authority<P> {
    /// Retain one client owner in an anonymous file without installing a descriptor.
    ///
    /// Cloned file references share this owner. Final release destroys it before dropping
    /// the file's ordinary authority reference. Other clients and the independent revocation
    /// owner remain usable; final authority release still performs normal cleanup.
    ///
    /// Failure drops `owner` outside native admission locks, too. Do not retain this file
    /// in its own owner. Call outside every lock needed by owner or authority cleanup.
    /// The file observes completed revocation through poll and supports optional kernel
    /// description and stream-lifetime operations. It exposes no mapping or primary-node
    /// operations. Its hangup is not device-work completion.
    pub fn create_client_file<O: ClientOwner>(&self, owner: O) -> Result<ARef<File>> {
        let data = KBox::into_raw(KBox::new(owner, GFP_KERNEL)?);
        // SAFETY: Authority remains live. OPS describes the exact allocation and its module;
        // native success consumes data, while failure leaves ownership with this caller.
        let result = from_err_ptr(unsafe {
            bindings::drm_capture_client_file_create(
                self.raw.get(),
                &Callbacks::<O>::OPS,
                data.cast(),
            )
        });
        match result {
            Ok(file) => {
                // SAFETY: Creation returns one initialized, unpublished file reference, without
                // concurrent fdget_pos operations violating the thread-safe File invariant.
                Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(file.cast())) })
            }
            Err(error) => {
                // SAFETY: Failure neither consumes the allocation nor invokes its callback.
                drop(unsafe { KBox::from_raw(data) });
                Err(error)
            }
        }
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
