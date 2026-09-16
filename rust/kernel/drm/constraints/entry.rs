// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Device-lifetime identities retaining immutable descriptions and typed backends.

use super::Description;
use crate::{
    error::from_err_ptr,
    prelude::*,
    sync::{
        aref::{
            ARef,
            AlwaysRefCounted, //
        },
        Arc,
        ArcBorrow, //
    },
    types::{
        ForeignOwnable,
        Opaque, //
    }, //
};
use core::{
    ffi::c_void,
    marker::PhantomData,
    ops::Deref,
    ptr::NonNull, //
};

/// Bounded identity namespace shared by every output for one device lifetime.
///
/// Retained entries keep the domain alive and remain charged to its limit. IDs are never reused.
/// The domain does not retain a DRM device or establish modesetting authority. All operations
/// and final reference release require a context that may sleep.
///
/// # Invariants
///
/// Every reference retains an initialized native domain with a live reference count.
#[repr(transparent)]
pub struct Domain(pub(super) Opaque<bindings::drm_constraints_domain>);

// SAFETY: Native reference counting and accounting are synchronized.
unsafe impl Send for Domain {}
// SAFETY: Shared operations use native synchronization.
unsafe impl Sync for Domain {}

// SAFETY: Native get/put maintain the initialized allocation.
unsafe impl AlwaysRefCounted for Domain {
    fn inc_ref(&self) {
        // SAFETY: The shared reference proves the native domain remains live.
        unsafe { bindings::drm_constraints_domain_get(self.0.get()) };
    }

    unsafe fn dec_ref(ptr: NonNull<Self>) {
        // SAFETY: The caller transfers a reference to the identically represented native object.
        unsafe { bindings::drm_constraints_domain_put(ptr.as_ptr().cast()) };
    }
}

impl Domain {
    /// Allocate a namespace with a nonzero retained-entry limit.
    ///
    /// A provider must keep one namespace across the device lifetime, not create one per offer.
    pub fn new(limit: u32) -> Result<ARef<Self>> {
        // SAFETY: Creation accepts a scalar and returns an owned native domain or an error.
        let raw = from_err_ptr(unsafe { bindings::drm_constraints_domain_create(limit) })?;
        // SAFETY: Successful construction transfers one non-null initialized reference.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(raw.cast())) })
    }
}

/// Provider resources implementing an immutable constraints entry.
///
/// Keeping a backend alive does not establish availability, activate it or grant source access.
/// Destruction may sleep and runs after the final native entry reference is released.
///
/// # Safety
///
/// `OwnerModule` must retain the code and static data used to destroy the backend. The `vtable`
/// implementation attribute selects the implementing module by default.
#[vtable]
pub unsafe trait Backend: Send + Sync + 'static {}

/// Immutable entry metadata without access to provider-private resources.
///
/// Retaining an entry preserves its description and any backend resources until final release.
/// It does not establish readiness, select the entry or authorize source reads. Operations and
/// final reference release require a context that may sleep.
///
/// # Invariants
///
/// Every reference retains a live native entry. Native ownership retains any release callback
/// and its module independently of the Rust view's type.
#[repr(transparent)]
pub struct OpaqueEntry(Opaque<bindings::drm_constraints_entry>);

// SAFETY: Native entry ownership and immutable metadata are synchronized independently of type.
unsafe impl Send for OpaqueEntry {}
// SAFETY: No provider-private data can be accessed through this view.
unsafe impl Sync for OpaqueEntry {}

// SAFETY: Native get/put retain the allocation, description and any provider resources.
unsafe impl AlwaysRefCounted for OpaqueEntry {
    fn inc_ref(&self) {
        // SAFETY: The shared reference retains an initialized native entry.
        unsafe { bindings::drm_constraints_entry_get(self.0.get()) };
    }

    unsafe fn dec_ref(ptr: NonNull<Self>) {
        // SAFETY: The caller transfers one reference to the transparently represented entry.
        unsafe { bindings::drm_constraints_entry_put(ptr.as_ptr().cast()) };
    }
}

impl OpaqueEntry {
    /// Create an entry for a backend with no private per-entry resources.
    ///
    /// Common DRM owns destruction, so a fixed default does not pin its provider's module.
    /// The device and accepted state must independently retain any resources used for execution.
    /// Resource-owning providers must use [`Entry::new`] instead.
    pub fn new_stateless(
        domain: &Domain,
        crtc_id: u32,
        description: &Description,
    ) -> Result<ARef<Self>> {
        // SAFETY: Construction borrows live inputs and returns an owned entry or an error.
        let raw = from_err_ptr(unsafe {
            bindings::drm_constraints_entry_create_stateless(
                domain.0.get(),
                crtc_id,
                description.0.get(),
            )
        })?;
        // SAFETY: Successful construction transfers one initialized native reference.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(raw.cast())) })
    }

    /// Positive identity, unique for the domain's lifetime.
    pub fn id(&self) -> u64 {
        // SAFETY: The shared reference retains the entry's immutable metadata.
        unsafe { bindings::drm_constraints_entry_id(self.0.get()) }
    }

    /// CRTC object ID, not modesetting authority or a retained object reference.
    pub fn crtc_id(&self) -> u32 {
        // SAFETY: The shared reference retains the entry's immutable metadata.
        unsafe { bindings::drm_constraints_entry_crtc(self.0.get()) }
    }

    /// Test exact namespace membership, independent of numeric ID equality.
    pub fn in_domain(&self, domain: &Domain) -> bool {
        // SAFETY: Both native references remain initialized and live throughout the comparison.
        unsafe { bindings::drm_constraints_entry_in_domain(self.0.get(), domain.0.get()) }
    }

    /// Borrow the immutable description retained by the native entry.
    pub fn description(&self) -> &Description {
        // SAFETY: Native ownership retains an initialized description for the entry's lifetime.
        unsafe { &*bindings::drm_constraints_entry_description(self.0.get()).cast() }
    }
}

/// Immutable entry retaining its domain, description and typed provider resources.
///
/// All operations and final reference release require a context that may sleep.
///
/// # Invariants
///
/// Every reference retains a live native entry created with `B`'s operations and a foreign-owned
/// `Arc<B>`. The native entry retains the callback module through final release.
#[repr(transparent)]
pub struct Entry<B: Backend> {
    pub(super) raw: Opaque<bindings::drm_constraints_entry>,
    _backend: PhantomData<B>,
}

// SAFETY: Native references are thread-safe and B permits transfer between tasks.
unsafe impl<B: Backend> Send for Entry<B> {}
// SAFETY: Native metadata is immutable and B permits shared access.
unsafe impl<B: Backend> Sync for Entry<B> {}

// SAFETY: Native get/put maintain the allocation and independently retained backend.
unsafe impl<B: Backend> AlwaysRefCounted for Entry<B> {
    fn inc_ref(&self) {
        // SAFETY: The shared reference proves the native entry remains live.
        unsafe { bindings::drm_constraints_entry_get(self.raw.get()) };
    }

    unsafe fn dec_ref(ptr: NonNull<Self>) {
        // SAFETY: The caller transfers a reference to the identically represented native entry.
        unsafe { bindings::drm_constraints_entry_put(ptr.as_ptr().cast()) };
    }
}

impl<B: Backend> Entry<B> {
    const OPS: bindings::drm_constraints_entry_ops = bindings::drm_constraints_entry_ops {
        owner: crate::module::this_module::<B::OwnerModule>().as_ptr(),
        release: Some(Self::release_callback),
    };

    unsafe extern "C" fn release_callback(data: *mut c_void) {
        // SAFETY: Final native release transfers its foreign Arc exactly once.
        drop(unsafe { Arc::<B>::from_foreign(data) });
    }

    /// Retain a description and backend under a never-reused identity.
    ///
    /// The provider separately validates the CRTC's membership and readiness before offering
    /// this entry. Failure drops the supplied backend reference and leaves borrowed inputs intact.
    pub fn new(
        domain: &Domain,
        crtc_id: u32,
        description: &Description,
        backend: Arc<B>,
    ) -> Result<ARef<Self>> {
        let data = backend.into_foreign();
        // SAFETY: Inputs remain live throughout construction. The promoted ops are retained by
        // B's module; native creation consumes the foreign Arc only on success.
        let raw = unsafe {
            bindings::drm_constraints_entry_create(
                domain.0.get(),
                crtc_id,
                description.0.get(),
                &Self::OPS,
                data,
            )
        };
        let raw = match from_err_ptr(raw) {
            Ok(raw) => raw,
            Err(error) => {
                // SAFETY: Failed native construction neither consumes nor releases the Arc.
                drop(unsafe { Arc::<B>::from_foreign(data) });
                return Err(error);
            }
        };
        // SAFETY: Successful construction transfers one initialized, correctly typed reference.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(raw.cast())) })
    }

    /// Borrow the backend retained by this exact entry, not a mutable current-backend pointer.
    pub fn backend(&self) -> ArcBorrow<'_, B> {
        // SAFETY: The type invariant establishes a foreign Arc<B>, retained throughout the borrow.
        unsafe { Arc::<B>::borrow(bindings::drm_constraints_entry_data(self.raw.get())) }
    }
}

impl<B: Backend> Deref for Entry<B> {
    type Target = OpaqueEntry;

    fn deref(&self) -> &OpaqueEntry {
        // SAFETY: Both types transparently represent the same live native entry. The opaque
        // view preserves native ownership without exposing or changing the backend's type.
        unsafe { &*self.raw.get().cast() }
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
