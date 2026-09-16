// SPDX-License-Identifier: GPL-2.0 OR MIT

//! KMS constraints attachment and scoped provider publication.

use super::{
    atomic::AtomicStateReader,
    crtc::{AsRawCrtc, Crtc, FromRawCrtcState, OpaqueCrtcState, UnregisteredCrtc},
    KmsDriver, ModeObject, UnregisteredKmsDevice,
};
use crate::{
    drm::{
        constraints::{Domain, List, OpaqueEntry, Snapshot},
        device::{Device, Registered},
    },
    error::to_result,
    prelude::*,
    sync::aref::ARef,
};
use core::ptr::NonNull;

/// Borrowed provider control for one initialized output's constraints list.
///
/// The registration or private test-device borrow excludes teardown. Operations require a
/// sleepable context and must not reenter from validation or acceptance callbacks holding the
/// list lock. Publication validates native object scope but the provider must establish backend
/// readiness and its own authority policy. These operations do not grant modesetting authority.
pub struct Output<'a, T: KmsDriver> {
    crtc: &'a Crtc<T::Crtc>,
    list: &'a List,
}

impl<T: KmsDriver> Device<T, Registered> {
    /// Borrow the provider control for an attached CRTC on this registered device.
    pub fn constraints_output<'a>(&'a self, crtc: &'a Crtc<T::Crtc>) -> Result<Output<'a, T>> {
        if self.as_raw() != crtc.drm_dev().as_raw() {
            return Err(EINVAL);
        }
        Output::new(crtc)
    }
}

impl<'a, T: KmsDriver> Output<'a, T> {
    // Only registered devices and private test owners expose this constructor. Crtc's runtime
    // view guarantees completed setup; the caller's borrow excludes mode configuration cleanup.
    pub(super) fn new(crtc: &'a Crtc<T::Crtc>) -> Result<Self> {
        // SAFETY: The runtime CRTC owns completed setup. The attached list pointer is immutable
        // until CRTC cleanup, which cannot occur during the owner's borrowed lifetime.
        let raw = NonNull::new(unsafe { bindings::drm_constraints_crtc_list(crtc.as_raw()) })
            .ok_or(EOPNOTSUPP)?;
        Ok(Self {
            crtc,
            // SAFETY: List transparently represents the live native list, borrowed from crtc.
            list: unsafe { raw.cast::<List>().as_ref() },
        })
    }

    /// Borrow the attached output's device-lifetime identity namespace.
    pub fn domain(&self) -> &Domain {
        // SAFETY: Attachment requires an initialized domain, retained by the borrowed device.
        unsafe { &*bindings::drm_constraints_device_domain(self.crtc.drm_dev().as_raw()).cast() }
    }

    /// Borrow the fixed default independently of current selection or offer availability.
    pub fn default_entry(&self) -> &OpaqueEntry {
        // SAFETY: Successful attachment retains a non-null fixed default until CRTC cleanup.
        unsafe { &*bindings::drm_constraints_crtc_default(self.crtc.as_raw()).cast() }
    }

    /// Restore a disabled, plane-free output's fixed default through atomic validation.
    ///
    /// The caller must first revoke departing source access and exclude new owners and
    /// competing updates through completion. Do not hold modeset locks or locks needed by
    /// source readers. This method does not establish that authority gate or disable scanout.
    ///
    /// An active output returns `EBUSY`. A changed binding requires an available default and
    /// successful provider checks; failure leaves the accepted binding unchanged. An already
    /// selected default in an open list is a no-op, not a promise of future readiness. Closed
    /// lists return `ESTALE`, including when the default is already selected, and stay closed.
    pub fn restore_default(&self) -> Result {
        // SAFETY: The output borrow retains initialized topology and excludes cleanup. Native
        // requests own validation, preparation and modeset locking throughout the operation.
        to_result(unsafe { bindings::drm_atomic_constraints_restore_default(self.crtc.as_raw()) })
    }

    /// Offer a ready backend after native device, output, plane and property scope checks.
    ///
    /// Adding an entry changes neither the accepted binding nor current buffer validity.
    pub fn add(&self, entry: &OpaqueEntry) -> Result {
        // SAFETY: The owner retains completed topology and the candidate throughout publication.
        to_result(unsafe { bindings::drm_constraints_crtc_add(self.crtc.as_raw(), entry.as_raw()) })
    }

    /// Copy a coherent snapshot; nonzero expected generation must match.
    pub fn snapshot(&self, generation: u64) -> Result<Snapshot> {
        self.list.snapshot(generation)
    }

    /// Resolve a selectable ID without reserving later acceptance or granting source access.
    pub fn lookup(&self, id: u64) -> Result<ARef<OpaqueEntry>> {
        self.list.lookup(id)
    }

    /// Retain accepted selection, including after closure; this is not presentation completion.
    pub fn selected(&self) -> ARef<OpaqueEntry> {
        self.list.selected()
    }

    /// Suggest an available entry, or clear the suggestion with zero. Never selects an entry.
    pub fn suggest(&self, id: u64) -> Result {
        self.list.suggest(id)
    }

    /// Withdraw an offer without undoing accepted work or completing native reads.
    pub fn withdraw(&self, id: u64) -> Result {
        self.list.withdraw(id)
    }

    /// Forget a withdrawn, unselected offer without discarding independent retained references.
    pub fn forget(&self, id: u64) -> Result {
        self.list.forget(id)
    }

    /// Permanently close publication and selection, not a reversible owner-interval reset.
    ///
    /// Existing references and native source-read obligations remain live. Closing a list does
    /// not quiesce its output or restore the fixed default.
    pub fn close(&self) {
        self.list.close();
    }
}

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
