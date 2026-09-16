// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Attach retained constraints to KMS outputs during exclusive device setup.

use super::{
    atomic::AtomicStateReader,
    crtc::{AsRawCrtc, FromRawCrtcState, OpaqueCrtcState, UnregisteredCrtc},
    KmsDriver, UnregisteredKmsDevice,
};
use crate::{
    drm::constraints::{Domain, OpaqueEntry},
    error::to_result,
    prelude::*,
    sync::aref::ARef,
};
use core::ptr::NonNull;

struct OutputOps<T: KmsDriver>(core::marker::PhantomData<T>);

impl<T: KmsDriver> OutputOps<T> {
    const OPS: bindings::drm_constraints_output_ops = bindings::drm_constraints_output_ops {
        check: Some(Self::check),
    };

    unsafe extern "C" fn check(
        state: *const bindings::drm_atomic_commit,
        crtc: *const bindings::drm_crtc_state,
        entry: *const bindings::drm_constraints_entry,
    ) -> i32 {
        // SAFETY: Attachment installs these operations only on T's CRTCs. Native validation
        // retains the transaction and entry, holds modeset/list locks, and stabilizes proposed
        // object states for this read-only callback, before hardware completion can occur.
        let state =
            unsafe { AtomicStateReader::<T>::new(NonNull::new_unchecked(state.cast_mut())) };
        // SAFETY: The callback supplies T's initialized CRTC state for the callback lifetime.
        let crtc = unsafe { OpaqueCrtcState::<T>::from_raw(crtc) };
        // SAFETY: The opaque view has native entry layout and borrows the retained allocation.
        let entry = unsafe { &*entry.cast::<OpaqueEntry>() };
        T::constraints_check(&state, crtc, entry)
            .err()
            .map_or(0, Error::to_errno)
    }
}

impl<T: KmsDriver> UnregisteredKmsDevice<'_, T> {
    /// Enable a bounded device-lifetime namespace before creating any CRTCs.
    ///
    /// The returned domain is retained independently of device teardown. It provides identity
    /// and quota accounting, not authority or a device reference. No UAPI is published.
    pub fn enable_constraints(&self, capacity: u32) -> Result<ARef<Domain>> {
        if !T::HAS_CONSTRAINTS_CHECK {
            return Err(EOPNOTSUPP);
        }
        // SAFETY: The setup view excludes concurrent registration, topology changes and cleanup.
        // Native initialization rejects repeated or late setup; mode configuration owns cleanup.
        to_result(unsafe { bindings::drm_constraints_device_init(self.as_raw(), capacity) })?;
        // SAFETY: Successful initialization installs a live domain. The device retains it while
        // get acquires the additional reference transferred to the Rust owner below.
        let domain = unsafe {
            bindings::drm_constraints_domain_get(bindings::drm_constraints_device_domain(
                self.as_raw(),
            ))
        };
        // SAFETY: Domain transparently represents the initialized non-null native allocation.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(domain.cast())) })
    }

    /// Attach an output's ready fixed default and bounded constraints list during setup.
    ///
    /// Construct every plane and property mentioned by the description first. Native checks
    /// validate device, CRTC and existing allocation/property scope. The default remains retained
    /// through the CRTC lifetime; state reset preserves accepted selection. Attachment neither
    /// activates the backend nor publishes userspace discovery. Failure retains no references.
    pub fn attach_constraints(
        &self,
        crtc: &UnregisteredCrtc<T::Crtc>,
        initial: &OpaqueEntry,
        capacity: u32,
    ) -> Result {
        // SAFETY: The borrowed setup CRTC is initialized and retains its immutable device link.
        if self.as_raw() != unsafe { (*crtc.as_raw()).dev } {
            return Err(EINVAL);
        }
        if !T::HAS_CONSTRAINTS_CHECK {
            return Err(EOPNOTSUPP);
        }
        // SAFETY: Exclusive setup retains T's initialized device and matching CRTC. Rust KMS
        // uses common atomic state/install helpers. The promoted operations live with T's KMS
        // callbacks through CRTC cleanup; native initialization retains initial only on success.
        to_result(unsafe {
            bindings::drm_constraints_crtc_init(
                crtc.as_raw(),
                initial.as_raw(),
                capacity,
                &OutputOps::<T>::OPS,
            )
        })
    }
}
