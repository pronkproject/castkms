// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Scoped software-state installation after native predecessor waits.

use super::{with_commit_scope, AtomicStateReader, CommitScope, KmsDriver};
use crate::{bindings, error::Error};
use core::{
    ffi::{c_int, c_void},
    ptr::NonNull,
};

type Continuation = unsafe extern "C" fn(*mut bindings::drm_atomic_commit, *mut c_void) -> c_int;

/// One opportunity to install a validated transaction, or reject it without mutation.
///
/// Native modeset locks remain held. Drivers may acquire their own inner locks,
/// revalidate external policy, and retain those locks through `install`. Neither
/// this value nor its state references may escape the callback. No waits, mutable
/// state access, or source authority are provided.
pub struct Install<'a, T: KmsDriver> {
    state: AtomicStateReader<T>,
    raw: NonNull<bindings::drm_atomic_commit>,
    continuation: Continuation,
    data: *mut c_void,
    scope: CommitScope<'a, T>,
}

/// Opaque outcome tied to one installation callback.
///
/// Only consuming that callback's `Install` can produce an outcome. Keeping the
/// native result opaque prevents reporting failure after a successful swap or
/// substituting success from another transaction.
///
/// ```compile_fail
/// use kernel::drm::kms::{atomic::{Install, InstallResult}, KmsDriver};
/// fn unrelated<'a, 'b, T: KmsDriver>(first: Install<'a, T>, second: Install<'b, T>)
///     -> InstallResult<'a, T>
/// {
///     drop(first);
///     second.install()
/// }
/// ```
#[must_use = "return the unchanged outcome to the native installation callback"]
pub struct InstallResult<'a, T: KmsDriver> {
    result: c_int,
    _scope: CommitScope<'a, T>,
}

impl<'a, T: KmsDriver> Install<'a, T> {
    /// Borrow candidate and preceding states while native modeset locks are held.
    pub fn state(&self) -> &AtomicStateReader<T> {
        &self.state
    }

    /// Reject without invoking native installation or consuming preparation.
    pub fn reject(self, error: Error) -> InstallResult<'a, T> {
        InstallResult {
            result: error.to_errno(),
            _scope: self.scope,
        }
    }

    /// Install inside the caller's serialization scope, at most once.
    ///
    /// Attached preparation is revalidated by the native continuation and may
    /// still reject the swap. The result must be returned unchanged to DRM.
    pub fn install(self) -> InstallResult<'a, T> {
        // SAFETY: The native callback supplies a scoped continuation and context
        // for this exact state. Consuming self prevents another invocation.
        let result = unsafe { (self.continuation)(self.raw.as_ptr(), self.data) };
        InstallResult {
            result,
            _scope: self.scope,
        }
    }
}

pub(in crate::drm::kms) unsafe extern "C" fn install_callback<T: KmsDriver>(
    raw: *mut bindings::drm_atomic_commit,
    continuation: Option<Continuation>,
    data: *mut c_void,
) -> c_int {
    let Some(raw) = NonNull::new(raw) else {
        return crate::error::code::EINVAL.to_errno();
    };
    let Some(continuation) = continuation else {
        return crate::error::code::EINVAL.to_errno();
    };
    with_commit_scope(|scope| {
        let install = Install {
            // SAFETY: Native installation retains the candidate and old states
            // throughout this callback, before any commit-tail worker may run.
            state: unsafe { AtomicStateReader::new(raw) },
            raw,
            continuation,
            data,
            scope,
        };
        T::atomic_commit_install(install).result
    })
}
