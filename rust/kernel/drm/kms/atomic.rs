// SPDX-License-Identifier: GPL-2.0 OR MIT

//! [`struct drm_atomic_commit`] related bindings for rust.
//!
//! [`struct drm_atomic_commit`]: srctree/include/drm/drm_atomic.h
use super::{connector::*, crtc::*, plane::*, KmsDriver, ModeObject};
use crate::{
    bindings,
    drm::device::{Device, Registered},
    error::{from_err_ptr, to_result},
    prelude::*,
    sync::aref::{ARef, AlwaysRefCounted},
    types::*,
};
use core::{cell::Cell, marker::*, mem::ManuallyDrop, ops::*, ptr::NonNull};

// The acquire context contains intrusive lists and belongs to its initializing task. Keep it
// pinned inside the transaction runner, where neither it nor its locks can escape the callback.
#[pin_data(PinnedDrop)]
struct ModesetAcquireContext {
    #[pin]
    raw: Opaque<bindings::drm_modeset_acquire_ctx>,
    _task: NotThreadSafe,
}

impl ModesetAcquireContext {
    fn new() -> impl PinInit<Self> {
        pin_init!(Self {
            raw <- Opaque::ffi_init(|slot| {
                // SAFETY: The slot is pinned, writable storage for the acquire context.
                unsafe { bindings::drm_modeset_acquire_init(slot, 0) };
            }),
            _task: NotThreadSafe,
        })
    }
}

#[pinned_drop]
impl PinnedDrop for ModesetAcquireContext {
    fn drop(self: Pin<&mut Self>) {
        // SAFETY: The context is initialized on this task and outlives every transaction using
        // it. All temporary state has been released before dropping locks and finalizing it.
        unsafe {
            bindings::drm_modeset_drop_locks(self.raw.get());
            bindings::drm_modeset_acquire_fini(self.raw.get());
        }
    }
}

impl<T: KmsDriver> Device<T, Registered> {
    /// Build and submit a blocking atomic update from the kernel.
    ///
    /// The callback edits a private transaction, using the same object-state helpers as driver
    /// callbacks. The framework owns the acquire context, validation, commit and cleanup; no
    /// userspace file or ioctl is involved. The registered-device borrow excludes unplug.
    ///
    /// Lock contention may discard an attempt and invoke `update` again with fresh state. The
    /// callback must propagate errors and be replayable: defer external side effects until this
    /// method succeeds. Do not enter with modeset locks held or recursively submit updates from
    /// the callback. State guards and references cannot escape the callback.
    pub fn atomic_update(
        &self,
        update: impl FnMut(Pin<&mut AtomicStateComposer<T>>) -> Result,
    ) -> Result {
        // SAFETY: Registration proves completed KMS initialization and excludes teardown.
        unsafe { run_update(self, update) }
    }
}

/// Run an update on an initialized device, also used by the unregistered runtime consumer.
///
/// # Safety
///
/// KMS setup, including initial object states, must have completed. The caller must exclude
/// mode-object creation, device registration and teardown throughout this call.
pub(super) unsafe fn run_update<T: KmsDriver>(
    dev: &Device<T>,
    mut update: impl FnMut(Pin<&mut AtomicStateComposer<T>>) -> Result,
) -> Result {
    pin_init::stack_pin_init!(let ctx = ModesetAcquireContext::new());
    loop {
        let (result, retry) = {
            // SAFETY: The caller guarantees completed KMS initialization and a live device.
            let raw = NonNull::new(unsafe { bindings::drm_atomic_commit_alloc(dev.as_raw()) })
                .ok_or(ENOMEM)?;
            // SAFETY: This unpublished allocation is exclusively owned by the runner.
            unsafe { (*raw.as_ptr()).acquire_ctx = ctx.raw.get() };
            // SAFETY: Transfer the allocation's reference to the composer with its initialized
            // acquire context already attached. Its drop runs before the context is destroyed.
            // No access to this state survives the callback or commit.
            let mut state = core::pin::pin!(unsafe { AtomicStateComposer::<T>::new(raw) });
            let result = update(state.as_mut());
            // A callback that accidentally consumes EDEADLK must still back off, never commit a
            // partial transaction. Only actual contention calls the native slow-lock path.
            // SAFETY: The initialized context belongs exclusively to this task.
            let contended = unsafe { !(*ctx.raw.get()).contended.is_null() };
            let result = if contended {
                Err(EDEADLK)
            } else {
                result.and_then(|()| {
                    // SAFETY: All callback borrows have ended. The transaction is unpublished and
                    // holds the required locks; the core performs validation before publishing it.
                    to_result(unsafe { bindings::drm_atomic_commit(raw.as_ptr()) })
                })
            };
            // The driver may discover contention during validation, after the callback returned.
            // SAFETY: Validation uses this task's initialized context synchronously.
            let retry = unsafe { !(*ctx.raw.get()).contended.is_null() };
            // SAFETY: No operation uses the acquire pointer after commit returns. Remove the
            // stack pointer even if DRM still holds another reference to the completed transaction.
            unsafe { (*raw.as_ptr()).acquire_ctx = core::ptr::null_mut() };
            // The pinned owner is dropped in place before backoff releases its acquire context.
            (result, retry)
        };
        if retry {
            // SAFETY: Temporary states are gone, and this task owns a contended acquire context.
            to_result(unsafe { bindings::drm_modeset_backoff(ctx.raw.get()) })?;
        } else {
            return result;
        }
    }
}

/// The main wrapper around [`struct drm_atomic_commit`].
///
/// This type is usually embedded within another interface such as an [`AtomicStateMutator`].
///
/// # Invariants
///
/// - The data layout of this type is identical to [`struct drm_atomic_commit`].
/// - `state` is initialized for as long as this type is exposed to users.
///
/// [`struct drm_atomic_commit`]: srctree/include/drm/drm_atomic.h
#[repr(transparent)]
pub struct AtomicState<T: KmsDriver> {
    pub(super) state: Opaque<bindings::drm_atomic_commit>,
    _p: PhantomData<T>,
}

impl<T: KmsDriver> AtomicState<T> {
    /// Reconstruct an immutable reference to an atomic state from the given pointer
    ///
    /// # Safety
    ///
    /// `ptr` must point to a valid initialized instance of [`struct drm_atomic_commit`].
    ///
    /// [`struct drm_atomic_commit`]: srctree/include/drm/drm_atomic.h
    #[allow(dead_code)]
    pub(super) unsafe fn from_raw<'a>(ptr: *const bindings::drm_atomic_commit) -> &'a Self {
        // SAFETY: Our data layout is identical
        // INVARIANT: Our safety contract upholds the guarantee that `state` is initialized for as
        // long as this type is exposed to users.
        unsafe { &*ptr.cast() }
    }

    pub(crate) fn as_raw(&self) -> *mut bindings::drm_atomic_commit {
        self.state.get()
    }

    /// Return the [`Device`] associated with this [`AtomicState`].
    pub fn drm_dev(&self) -> &Device<T> {
        // SAFETY:
        // - `state` is initialized via our type invariants.
        // - `dev` is invariant throughout the lifetime of `AtomicState`
        unsafe { Device::from_raw((*self.state.get()).dev) }
    }

    /// Return the old atomic state for `crtc`, if it is present within this [`AtomicState`].
    pub fn get_old_crtc_state<C>(&self, crtc: &C) -> Option<&C::State>
    where
        C: ModesettableCrtc + ModeObject<Driver = T>,
    {
        if !core::ptr::eq(self.drm_dev(), crtc.drm_dev()) {
            return None;
        }
        // SAFETY: The CRTC belongs to this device, so its index addresses this transaction.
        // This function either returns NULL or a valid pointer to a `drm_crtc_state`.
        unsafe {
            bindings::drm_atomic_get_old_crtc_state(self.as_raw(), crtc.as_raw())
                .as_ref()
                .map(|p| C::State::from_raw(p))
        }
    }

    /// Return the old atomic state for `plane`, if it is present within this [`AtomicState`].
    pub fn get_old_plane_state<P>(&self, plane: &P) -> Option<&P::State>
    where
        P: ModesettablePlane + ModeObject<Driver = T>,
    {
        if !core::ptr::eq(self.drm_dev(), plane.drm_dev()) {
            return None;
        }
        // SAFETY: The plane belongs to this device, so its index addresses this transaction.
        // This function either returns NULL or a valid pointer to a `drm_plane_state`.
        unsafe {
            bindings::drm_atomic_get_old_plane_state(self.as_raw(), plane.as_raw())
                .as_ref()
                .map(|p| P::State::from_raw(p))
        }
    }

    /// Return the old atomic state for `connector` if it is present within this [`AtomicState`].
    pub fn get_old_connector_state<C>(&self, connector: &C) -> Option<&C::State>
    where
        C: ModesettableConnector + ModeObject<Driver = T>,
    {
        if !core::ptr::eq(self.drm_dev(), connector.drm_dev()) {
            return None;
        }
        // SAFETY: The connector belongs to this device. This function either returns NULL or a
        // valid pointer to a `drm_connector_state`.
        unsafe {
            bindings::drm_atomic_get_old_connector_state(self.as_raw(), connector.as_raw())
                .as_ref()
                .map(|p| C::State::from_raw(p))
        }
    }

    /// Return the new state of the first connector routed to `crtc` in this [`AtomicState`], if
    /// any.
    ///
    /// This is the Rust spelling of walking `for_each_new_connector_in_state()` looking for
    /// `conn_state->crtc == crtc`, which is how a CRTC callback reaches the connector properties
    /// that describe the signal it is about to drive -- colorimetry, HDR metadata, `max bpc`.
    /// Those live on the connector state, but the driver decisions they feed are frequently made
    /// where only the CRTC is in hand.
    ///
    /// The state is returned opaquely because the caller is looking across mode objects and has
    /// no way to name the connector's driver-private state type. A CRTC that clones to several
    /// connectors gets the first; a driver that cares about the difference should walk the
    /// connectors itself.
    pub fn new_connector_state_for_crtc<C>(&self, crtc: &C) -> Option<&OpaqueConnectorState<T>>
    where
        C: ModesettableCrtc + ModeObject<Driver = T>,
    {
        let crtc_raw = crtc.as_raw();
        // SAFETY: `state` is initialized via our type invariants, and `connectors` /
        // `num_connector` are invariant for as long as we hold a reference to it.
        let (connectors, num) = unsafe {
            let raw = self.as_raw();
            ((*raw).connectors, (*raw).num_connector)
        };
        if connectors.is_null() || num <= 0 {
            return None;
        }
        for i in 0..num as usize {
            // SAFETY: `connectors` points to `num_connector` initialized entries.
            let new_state = unsafe { (*connectors.add(i)).new_state };
            if new_state.is_null() {
                continue;
            }
            // SAFETY: a non-null `new_state` is a valid `drm_connector_state` for the lifetime of
            // the atomic state.
            if unsafe { (*new_state).crtc } != crtc_raw {
                continue;
            }
            // SAFETY: as above, and the returned reference borrows from `self`, so it cannot
            // outlive the atomic state that owns the connector state.
            return Some(unsafe { OpaqueConnectorState::<T>::from_raw(new_state) });
        }
        None
    }

    /// Invoke `f` for every CRTC this [`AtomicState`] carries a new state for, passing the CRTC
    /// and that state.
    ///
    /// This is the Rust spelling of walking `for_each_new_crtc_in_state()`.
    ///
    /// A driver enforcing a constraint its heads share -- a bandwidth budget, a clock source, a
    /// fixed pool of scanout engines -- has to weigh what every head will be once the commit
    /// lands. A per-CRTC callback that consults committed state instead sees each sibling at its
    /// old value, so two heads that both rise in one commit each find the other still low, pass
    /// individually, and break the shared limit together.
    pub fn for_each_new_crtc_state<F>(&self, mut f: F)
    where
        F: FnMut(&Crtc<T::Crtc>, &OpaqueCrtcState<T>),
    {
        // SAFETY: `state` is initialized via our type invariants, and `crtcs` together with the
        // device's `num_crtc` are invariant for as long as we hold a reference to it.
        let (crtcs, num) = unsafe {
            let raw = self.as_raw();
            ((*raw).crtcs, (*(*raw).dev).mode_config.num_crtc)
        };
        if crtcs.is_null() || num <= 0 {
            return;
        }
        for i in 0..num as usize {
            // SAFETY: `crtcs` points to `num_crtc` initialized entries.
            let (ptr, new_state) = unsafe { ((*crtcs.add(i)).ptr, (*crtcs.add(i)).new_state) };
            if ptr.is_null() || new_state.is_null() {
                continue;
            }
            // SAFETY: every CRTC of a `KmsDriver` device is a `Crtc<T::Crtc>`, and a non-null
            // `new_state` is a valid `drm_crtc_state`. Both borrow from `self`, so neither can
            // outlive the atomic state owning them.
            let (crtc, state) = unsafe {
                (
                    Crtc::<T::Crtc>::from_raw(ptr),
                    OpaqueCrtcState::<T>::from_raw(new_state),
                )
            };
            f(crtc, state);
        }
    }

    /// Return the old state of the first connector routed to `crtc` in this [`AtomicState`], if
    /// any.
    ///
    /// The counterpart to [`Self::new_connector_state_for_crtc`], for a driver comparing the two
    /// to decide whether a connector property it consumes has changed.
    pub fn old_connector_state_for_crtc<C>(&self, crtc: &C) -> Option<&OpaqueConnectorState<T>>
    where
        C: ModesettableCrtc + ModeObject<Driver = T>,
    {
        let crtc_raw = crtc.as_raw();
        // SAFETY: `state` is initialized via our type invariants, and `connectors` /
        // `num_connector` are invariant for as long as we hold a reference to it.
        let (connectors, num) = unsafe {
            let raw = self.as_raw();
            ((*raw).connectors, (*raw).num_connector)
        };
        if connectors.is_null() || num <= 0 {
            return None;
        }
        for i in 0..num as usize {
            // SAFETY: `connectors` points to `num_connector` initialized entries.
            let old_state = unsafe { (*connectors.add(i)).old_state };
            if old_state.is_null() {
                continue;
            }
            // SAFETY: a non-null `old_state` is a valid `drm_connector_state` for the lifetime of
            // the atomic state.
            if unsafe { (*old_state).crtc } != crtc_raw {
                continue;
            }
            // SAFETY: as above, and the returned reference borrows from `self`, so it cannot
            // outlive the atomic state that owns the connector state.
            return Some(unsafe { OpaqueConnectorState::<T>::from_raw(old_state) });
        }
        None
    }
}

// SAFETY: DRM atomic state objects are always reference counted and the get/put functions satisfy
// the requirements.
unsafe impl<T: KmsDriver> AlwaysRefCounted for AtomicState<T> {
    fn inc_ref(&self) {
        // SAFETY: `state` is initialized for as long as this type is exposed to users
        unsafe { bindings::drm_atomic_commit_get(self.state.get()) }
    }

    unsafe fn dec_ref(obj: NonNull<Self>) {
        // SAFETY: `obj` contains a valid non-null pointer to an initialized `Self`.
        unsafe { bindings::drm_atomic_commit_put(obj.as_ptr().cast()) }
    }
}

/// Read-only object-state access during an atomic commit callback.
///
/// DRM publishes the new states before scheduling the commit worker. Another atomic check may
/// already be duplicating their private payloads, so commit callbacks must not acquire mutable
/// payload guards. This accessor is only borrowed from a callback; it deliberately does not expose
/// the reference-counted [`AtomicState`], since retaining that object would not retain its new
/// states after hardware completion.
pub struct AtomicStateReader<T: KmsDriver>(ManuallyDrop<ARef<AtomicState<T>>>);

impl<T: KmsDriver> AtomicStateReader<T> {
    /// # Safety
    ///
    /// `ptr` must be a commit for `T` supplied to a DRM commit callback. The reader must not
    /// outlive that callback, and hardware completion must not be signaled while it is in use.
    pub(super) unsafe fn new(ptr: NonNull<bindings::drm_atomic_commit>) -> Self {
        // SAFETY: The callback borrows DRM's reference, as required above. Suppress its put.
        Self(ManuallyDrop::new(unsafe { ARef::from_raw(ptr.cast()) }))
    }

    /// Return the device that owns the commit.
    pub fn drm_dev(&self) -> &Device<T> {
        self.0.drm_dev()
    }

    /// Return the old state of `crtc` if it belongs to this commit's device and is present.
    pub fn get_old_crtc_state<C>(&self, crtc: &C) -> Option<&C::State>
    where
        C: ModesettableCrtc + ModeObject<Driver = T>,
    {
        self.0.get_old_crtc_state(crtc)
    }

    /// Return the published new state of `crtc`, without granting mutable payload access.
    pub fn get_new_crtc_state<C>(&self, crtc: &C) -> Option<&C::State>
    where
        C: ModesettableCrtc + ModeObject<Driver = T>,
    {
        if !core::ptr::eq(self.drm_dev(), crtc.drm_dev()) {
            return None;
        }
        // SAFETY: The device check validates the index. The reader's callback scope guarantees
        // that a non-null new state is alive and its driver-private payload is shared read-only.
        let state = unsafe {
            bindings::drm_atomic_get_new_crtc_state(self.0.as_raw(), crtc.as_raw())
        };
        NonNull::new(state).map(|s| unsafe { C::State::from_raw(s.as_ptr()) })
    }

    /// Return the old state of `plane` if it belongs to this commit's device and is present.
    pub fn get_old_plane_state<P>(&self, plane: &P) -> Option<&P::State>
    where
        P: ModesettablePlane + ModeObject<Driver = T>,
    {
        self.0.get_old_plane_state(plane)
    }

    /// Return the published new state of `plane`, without granting mutable payload access.
    pub fn get_new_plane_state<P>(&self, plane: &P) -> Option<&P::State>
    where
        P: ModesettablePlane + ModeObject<Driver = T>,
    {
        if !core::ptr::eq(self.drm_dev(), plane.drm_dev()) {
            return None;
        }
        // SAFETY: The device check validates the index. The callback keeps the published state
        // alive, and neither this reader nor another commit callback grants mutable access.
        let state = unsafe {
            bindings::drm_atomic_get_new_plane_state(self.0.as_raw(), plane.as_raw())
        };
        NonNull::new(state).map(|s| unsafe { P::State::from_raw(s.as_ptr()) })
    }

    /// Return the old connector state, if present in this device's commit.
    pub fn get_old_connector_state<C>(&self, connector: &C) -> Option<&C::State>
    where
        C: ModesettableConnector + ModeObject<Driver = T>,
    {
        self.0.get_old_connector_state(connector)
    }

    /// Return the published new state of `connector`, without mutable payload access.
    pub fn get_new_connector_state<C>(&self, connector: &C) -> Option<&C::State>
    where
        C: ModesettableConnector + ModeObject<Driver = T>,
    {
        if !core::ptr::eq(self.drm_dev(), connector.drm_dev()) {
            return None;
        }
        // SAFETY: The same-device check validates the index. The callback keeps the
        // published state alive and private payload access is shared read-only.
        let state = unsafe {
            bindings::drm_atomic_get_new_connector_state(self.0.as_raw(), connector.as_raw())
        };
        NonNull::new(state).map(|s| unsafe { C::State::from_raw(s.as_ptr()) })
    }

    /// Invoke `f` for every CRTC with a published new state in this commit.
    pub fn for_each_new_crtc_state<F>(&self, f: F)
    where
        F: FnMut(&Crtc<T::Crtc>, &OpaqueCrtcState<T>),
    {
        self.0.for_each_new_crtc_state(f)
    }

    /// Return the new state of the first connector routed to `crtc`, if any.
    pub fn new_connector_state_for_crtc<C>(&self, crtc: &C) -> Option<&OpaqueConnectorState<T>>
    where
        C: ModesettableCrtc + ModeObject<Driver = T>,
    {
        self.0.new_connector_state_for_crtc(crtc)
    }

    /// Return the old state of the first connector routed to `crtc`, if any.
    pub fn old_connector_state_for_crtc<C>(&self, crtc: &C) -> Option<&OpaqueConnectorState<T>>
    where
        C: ModesettableCrtc + ModeObject<Driver = T>,
    {
        self.0.old_connector_state_for_crtc(crtc)
    }
}

/// A smart-pointer for modifying the contents of an atomic state.
///
/// As it's not unreasonable for a modesetting driver to want to have references to the state of
/// multiple modesetting objects at once, along with mutating multiple states for unique modesetting
/// objects at once, this type provides a mechanism for safely doing both of these things.
///
/// To honor Rust's aliasing rules regarding mutable references, this structure ensures only one
/// mutable reference to a mode object's atomic state may exist at a time - and refuses to provide
/// another if one has already been taken out using runtime checks.
///
/// Mutable access is restricted to atomic checking, before the new states are published.
/// Commit callbacks use [`AtomicStateReader`] instead, even before hardware completion.
pub struct AtomicStateMutator<T: KmsDriver> {
    /// The state being mutated. Note that the use of `ManuallyDrop` here is because mutators are
    /// only constructed in FFI callbacks and thus borrow their references to the atomic state from
    /// DRM. Composers, which make use of mutators internally, can potentially be owned by rust code
    /// if a driver is performing an atomic commit internally - and thus will call the drop
    /// implementation here.
    state: ManuallyDrop<ARef<AtomicState<T>>>,

    /// Bitmask of borrowed CRTC state objects
    pub(super) borrowed_crtcs: Cell<u32>,
    /// Bitmask of borrowed plane state objects
    pub(super) borrowed_planes: Cell<u32>,
    /// Bitmask of borrowed connector state objects
    pub(super) borrowed_connectors: Cell<u32>,
}

impl<T: KmsDriver> AtomicStateMutator<T> {
    /// Construct a new [`AtomicStateMutator`]
    ///
    /// # Safety
    ///
    /// `ptr` must point to a valid `drm_atomic_commit`
    #[allow(dead_code)]
    pub(super) unsafe fn new(ptr: NonNull<bindings::drm_atomic_commit>) -> Self {
        Self {
            // SAFETY: The data layout of `AtomicState<T>` is identical to drm_atomic_commit
            // We use `ManuallyDrop` because `AtomicStateMutator` is only ever provided to users in
            // the context of KMS callbacks. As such, skipping ref inc/dec for the atomic state is
            // convienent for our bindings.
            state: ManuallyDrop::new(unsafe { ARef::from_raw(ptr.cast()) }),
            borrowed_planes: Cell::default(),
            borrowed_crtcs: Cell::default(),
            borrowed_connectors: Cell::default(),
        }
    }

    pub(crate) fn as_raw(&self) -> *mut bindings::drm_atomic_commit {
        self.state.as_raw()
    }

    /// Return the [`Device`] for this [`AtomicStateMutator`].
    pub fn drm_dev(&self) -> &Device<T> {
        self.state.drm_dev()
    }

    /// Retrieve the last committed atomic state for `crtc` if `crtc` has already been added to the
    /// atomic state being composed.
    ///
    /// Returns `None` otherwise.
    pub fn get_old_crtc_state<C>(&self, crtc: &C) -> Option<&C::State>
    where
        C: ModesettableCrtc + ModeObject<Driver = T>,
    {
        self.state.get_old_crtc_state(crtc)
    }

    /// Retrieve the last committed atomic state for `connector` if `connector` has already been
    /// added to the atomic state being composed.
    ///
    /// Returns `None` otherwise.
    pub fn get_old_connector_state<C>(&self, connector: &C) -> Option<&C::State>
    where
        C: ModesettableConnector + ModeObject<Driver = T>,
    {
        self.state.get_old_connector_state(connector)
    }

    /// Return the new state of the first connector routed to `crtc`, if any.
    ///
    /// See [`AtomicState::new_connector_state_for_crtc`]. The exclusive borrow prevents any
    /// mutable state guard from coexisting with the returned shared state. An opaque state can
    /// be converted to a typed state, so it must obey the same borrowing rules.
    pub fn new_connector_state_for_crtc<C>(&mut self, crtc: &C) -> Option<&OpaqueConnectorState<T>>
    where
        C: ModesettableCrtc + ModeObject<Driver = T>,
    {
        self.state.new_connector_state_for_crtc(crtc)
    }

    /// Inspect the first new connector state routed to `crtc` during atomic checking.
    ///
    /// Unlike the exclusive-borrow lookup, this is callable through a check token's shared
    /// composer. The callback cannot retain the state reference. An outstanding connector
    /// guard or nested inspection returns `EBUSY`; connector mutation is excluded until the
    /// callback returns. A missing route is passed as `None`.
    pub fn with_new_connector_state_for_crtc<C, R>(
        &self,
        crtc: &C,
        inspect: impl FnOnce(Option<&OpaqueConnectorState<T>>) -> R,
    ) -> Result<R>
    where
        C: ModesettableCrtc + ModeObject<Driver = T>,
    {
        if self.borrowed_connectors.get() != 0 {
            return Err(EBUSY);
        }
        self.borrowed_connectors.set(u32::MAX);
        let _restore = ScopeGuard::new(|| self.borrowed_connectors.set(0));
        Ok(inspect(self.state.new_connector_state_for_crtc(crtc)))
    }

    /// Invoke `f` for every CRTC this state carries a new state for.
    ///
    /// See [`AtomicState::for_each_new_crtc_state`]. The exclusive borrow excludes outstanding
    /// mutable guards and prevents callbacks from reacquiring a guard through this mutator.
    pub fn for_each_new_crtc_state<F>(&mut self, f: F)
    where
        F: FnMut(&Crtc<T::Crtc>, &OpaqueCrtcState<T>),
    {
        self.state.for_each_new_crtc_state(f)
    }

    /// Inspect new CRTC states through a check token's shared composer.
    ///
    /// No CRTC guard may be outstanding. All CRTC slots remain borrowed throughout traversal,
    /// so reentrant mutation and nested traversal are rejected. References cannot escape `f`.
    /// This is conservative: even a guard for a different CRTC prevents traversal.
    pub fn try_for_each_new_crtc_state<F>(&self, f: F) -> Result
    where
        F: FnMut(&Crtc<T::Crtc>, &OpaqueCrtcState<T>),
    {
        if self.borrowed_crtcs.get() != 0 {
            return Err(EBUSY);
        }
        self.borrowed_crtcs.set(u32::MAX);
        let _restore = ScopeGuard::new(|| self.borrowed_crtcs.set(0));
        self.state.for_each_new_crtc_state(f);
        Ok(())
    }

    /// Return the old state of the first connector routed to `crtc`, if any.
    ///
    /// See [`AtomicState::old_connector_state_for_crtc`].
    pub fn old_connector_state_for_crtc<C>(&self, crtc: &C) -> Option<&OpaqueConnectorState<T>>
    where
        C: ModesettableCrtc + ModeObject<Driver = T>,
    {
        self.state.old_connector_state_for_crtc(crtc)
    }

    /// Retrieve the last committed atomic state for `plane` if `plane` has already been added to
    /// the atomic state being composed.
    ///
    /// Returns `None` otherwise.
    pub fn get_old_plane_state<P>(&self, plane: &P) -> Option<&P::State>
    where
        P: ModesettablePlane + ModeObject<Driver = T>,
    {
        self.state.get_old_plane_state(plane)
    }

    /// Return a composer for `plane`s new atomic state if it was previously added to the atomic
    /// state being composed.
    ///
    /// Returns `None` otherwise, or if another mutator still exists for this state.
    pub fn get_new_crtc_state<C>(&self, crtc: &C) -> Option<CrtcStateMutator<'_, C::State>>
    where
        C: ModesettableCrtc + ModeObject<Driver = T>,
    {
        if !core::ptr::eq(self.drm_dev(), crtc.drm_dev()) {
            return None;
        }
        // SAFETY: The CRTC belongs to this device, so its index addresses this transaction.
        // DRM either returns NULL or a valid pointer to a `drm_crtc_state`.
        let state =
            unsafe { bindings::drm_atomic_get_new_crtc_state(self.as_raw(), crtc.as_raw()) };

        CrtcStateMutator::<C::State>::new(self, NonNull::new(state)?)
    }

    /// Return a composer for `plane`s new atomic state if it was previously added to the atomic
    /// state being composed.
    ///
    /// Returns `None` otherwise, or if another mutator still exists for this state.
    pub fn get_new_plane_state<P>(&self, plane: &P) -> Option<PlaneStateMutator<'_, P::State>>
    where
        P: ModesettablePlane + ModeObject<Driver = T>,
    {
        if !core::ptr::eq(self.drm_dev(), plane.drm_dev()) {
            return None;
        }
        // SAFETY: The plane belongs to this device, so its index addresses this transaction.
        // DRM either returns NULL or a valid pointer to a `drm_plane_state`.
        let state =
            unsafe { bindings::drm_atomic_get_new_plane_state(self.as_raw(), plane.as_raw()) };

        PlaneStateMutator::<P::State>::new(self, NonNull::new(state)?)
    }

    /// Return a composer for `crtc`s new atomic state if it was previously added to the atomic
    /// state being composed.
    ///
    /// Returns `None` otherwise, or if another mutator still exists for this state.
    pub fn get_new_connector_state<C>(
        &self,
        connector: &C,
    ) -> Option<ConnectorStateMutator<'_, C::State>>
    where
        C: ModesettableConnector + ModeObject<Driver = T>,
    {
        if !core::ptr::eq(self.drm_dev(), connector.drm_dev()) {
            return None;
        }
        // SAFETY: The connector belongs to this device. DRM either returns NULL or a valid
        // pointer to a `drm_connector_state`.
        let state = unsafe {
            bindings::drm_atomic_get_new_connector_state(self.as_raw(), connector.as_raw())
        };

        ConnectorStateMutator::<C::State>::new(self, NonNull::new(state)?)
    }
}

/// An [`AtomicStateMutator`] wrapper which is not yet part of any commit operation.
///
/// Since it's not yet part of a commit operation, new mode objects may be added to the state. It
/// also holds a reference to the underlying [`AtomicState`] that will be released when this object
/// is dropped.
///
/// Kernel update callbacks receive a pinned borrow, not a movable transaction owner. Keeping
/// the owner fixed preserves the runner's association between state and acquire context.
pub struct AtomicStateComposer<T: KmsDriver>(AtomicStateMutator<T>, PhantomPinned);

impl<T: KmsDriver> Deref for AtomicStateComposer<T> {
    type Target = AtomicStateMutator<T>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: KmsDriver> Drop for AtomicStateComposer<T> {
    fn drop(&mut self) {
        // SAFETY: We're in drop, so this is guaranteed to be the last possible reference
        unsafe { ManuallyDrop::drop(&mut self.0.state) }
    }
}

impl<T: KmsDriver> AtomicStateComposer<T> {
    /// # Safety
    ///
    /// `ptr` must be an unpublished transaction for `T`, with an initialized acquire context
    /// owned by the current task. The caller must exclude other access to its new states while
    /// the composer is borrowed. Both the device and acquire context must outlive the composer.
    /// Transfer one owned reference, or suppress drop when borrowing a C callback's reference.
    pub(crate) unsafe fn new(ptr: NonNull<bindings::drm_atomic_commit>) -> Self {
        // SAFETY: The unpublished, exclusively accessed transaction satisfies the mutator's
        // requirements. The caller supplies ownership or suppresses the composer's destructor.
        Self(unsafe { AtomicStateMutator::new(ptr) }, PhantomPinned)
    }

    /// Attempt to add the state for `crtc` to the atomic state for this composer if it hasn't
    /// already been added, and create a mutator for it.
    ///
    /// If a composer already exists for this `crtc`, this function returns `Error(EBUSY)`. If
    /// attempting to add the state fails, another error code will be returned.
    pub fn add_crtc_state<C>(&self, crtc: &C) -> Result<CrtcStateMutator<'_, C::State>>
    where
        C: ModesettableCrtc + ModeObject<Driver = T>,
    {
        if !core::ptr::eq(self.drm_dev(), crtc.drm_dev()) {
            return Err(EINVAL);
        }
        // SAFETY: The CRTC belongs to this device. DRM returns a valid state pointer or an error.
        let state = unsafe {
            from_err_ptr(bindings::drm_atomic_get_crtc_state(
                self.as_raw(),
                crtc.as_raw(),
            ))
            .map(|c| NonNull::new_unchecked(c))
        }?;

        CrtcStateMutator::<C::State>::new(self, state).ok_or(EBUSY)
    }

    /// Attempt to add the state for `plane` to the atomic state for this composer if it hasn't
    /// already been added, and create a mutator for it.
    ///
    /// If a composer already exists for this `plane`, this function returns `Error(EBUSY)`. If
    /// attempting to add the state fails, another error code will be returned.
    pub fn add_plane_state<P>(&self, plane: &P) -> Result<PlaneStateMutator<'_, P::State>>
    where
        P: ModesettablePlane + ModeObject<Driver = T>,
    {
        if !core::ptr::eq(self.drm_dev(), plane.drm_dev()) {
            return Err(EINVAL);
        }
        // SAFETY: The plane belongs to this device. DRM returns a valid state pointer or an error.
        let state = unsafe {
            from_err_ptr(bindings::drm_atomic_get_plane_state(
                self.as_raw(),
                plane.as_raw(),
            ))
            .map(|p| NonNull::new_unchecked(p))
        }?;

        PlaneStateMutator::<P::State>::new(self, state).ok_or(EBUSY)
    }

    /// Attempt to add the state for `connector` to the atomic state for this composer if it hasn't
    /// already been added, and create a mutator for it.
    ///
    /// If a composer already exists for this `connector`, this function returns `Error(EBUSY)`. If
    /// attempting to add the state fails, another error code will be returned.
    pub fn add_connector_state<C>(
        &self,
        connector: &C,
    ) -> Result<ConnectorStateMutator<'_, C::State>>
    where
        C: ModesettableConnector + ModeObject<Driver = T>,
    {
        if !core::ptr::eq(self.drm_dev(), connector.drm_dev()) {
            return Err(EINVAL);
        }
        // SAFETY: The connector belongs to this device. DRM returns a valid state or an error.
        let state = unsafe {
            from_err_ptr(bindings::drm_atomic_get_connector_state(
                self.as_raw(),
                connector.as_raw(),
            ))
            .map(|c| NonNull::new_unchecked(c))
        }?;

        ConnectorStateMutator::<C::State>::new(self, state).ok_or(EBUSY)
    }

    /// Attempt to add any planes affected by changes on `crtc` to this [`AtomicStateComposer`].
    ///
    /// Will return an [`Error`] if this fails.
    pub fn add_affected_planes<C>(&self, crtc: &C) -> Result
    where
        C: ModesettableCrtc + ModeObject<Driver = T>,
    {
        if !core::ptr::eq(self.drm_dev(), crtc.drm_dev()) {
            return Err(EINVAL);
        }
        // SAFETY: Both pointers are valid and the CRTC belongs to the transaction's device.
        to_result(unsafe { bindings::drm_atomic_add_affected_planes(self.as_raw(), crtc.as_raw()) })
    }
}

/// A macro for declaring the repetitive take_all(), take_state(), etc. methods for atomic state
/// token types.
///
/// It is assumed that $token_name refers to a struct that contains two members:
///
/// - `state`: This should be the atomic state type to use
/// - The object in question. The name of this member is generated by converting $obj to lowercase.
///
/// The struct should have one lifetime ($lifetime_a) declared, and one meta-variable ($meta) which
/// should be bound to the Driver* trait for the given mode object.
macro_rules! impl_atomic_state_token_ops {
    (
        $token_name:ident,
        $state:ident,
        $obj:ident,
        $new_state:ty,
        use <$lifetime_a:lifetime, $meta:ident>
    ) => {
        kernel::macros::paste! {
            /// Create a new token.
            ///
            /// # Safety
            ///
            /// To use this function it must be known in the current context that:
            ///
            /// - The object has had its atomic states added to `state`.
            /// - No state mutator can possibly be taken out for the objects new state.
            pub(crate) unsafe fn new(
                [<$obj:lower>]: &$lifetime_a $obj<$meta>,
                state: &$lifetime_a $state<$meta::Driver>,
            ) -> Self {
                Self { [<$obj:lower>], state }
            }

            #[doc = concat!("Get the [`", stringify!($obj), "`] associated with this",
                            " [`", stringify!($token_name), "`].")]
            pub fn [<$obj:lower>](&self) -> &$lifetime_a $obj<$meta> {
                self.[<$obj:lower>]
            }

            /// Exchange this token for a (atomic_state, old_state, new_state) tuple.
            pub fn take_all(self) -> (
                &$lifetime_a $state<$meta::Driver>,
                &$lifetime_a [<$obj State>]<$meta::State>,
                $new_state,
            ) {
                let (old_state, new_state) = (
                    self.state.[<get_old_ $obj:lower _state>](self.[<$obj:lower>]),
                    self.state.[<get_new_ $obj:lower _state>](self.[<$obj:lower>]),
                );

                // SAFETY:
                // - Both the old and new object state are present in `state` via our type
                //   invariants.
                // - The new state is guaranteed to have no mutators taken out via our type
                //   invariants.
                let (old_state, new_state) = unsafe {
                    (old_state.unwrap_unchecked(), new_state.unwrap_unchecked())
                };

                (self.state, old_state, new_state)
            }

            #[doc = concat!("Exchange this token for the old [`", stringify!($obj), "State`].")]
            pub fn take_old_state(self) -> &$lifetime_a [<$obj State>]<$meta::State> {
                let old = self.state.[<get_old_ $obj:lower _state>](self.[<$obj:lower>]);

                // SAFETY: The old state is guaranteed to be present in `state` via our type
                // invariants.
                unsafe { old.unwrap_unchecked() }
            }

            #[doc = concat!("Exchange this token for the new [`", stringify!($obj), "State`].")]
            pub fn take_new_state(self) -> $new_state {
                let new = self.state.[<get_new_ $obj:lower _state>](self.[<$obj:lower>]);

                // SAFETY:
                // - The new state is guaranteed to be present in our `state` via our type
                //   invariants.
                // - The new state is guaranteed not to have any mutators taken out for it via our
                //   type invariants.
                unsafe { new.unwrap_unchecked() }
            }

            #[doc = concat!("Exchange this token for both the old and new [`",
                            stringify!($obj), "State`].")]
            pub fn take_old_new_state(self) -> (
                &$lifetime_a [<$obj State>]<$meta::State>,
                $new_state,
            ) {
                let (old_state, new_state) = (
                    self.state.[<get_old_ $obj:lower _state>](self.[<$obj:lower>]),
                    self.state.[<get_new_ $obj:lower _state>](self.[<$obj:lower>]),
                );

                // SAFETY:
                // - Both the old and new object state are present in `state` via our type
                //   invariants.
                // - The new state is guaranteed to have no mutators taken out via our type
                //   invariants.
                let (old_state, new_state) = unsafe {
                    (old_state.unwrap_unchecked(), new_state.unwrap_unchecked())
                };

                (old_state, new_state)
            }

            #[doc = concat!("Exchange this token for both the [`", stringify!($state),
                            "`] and the old [`", stringify!($obj), "State`].")]
            pub fn take_state_old_state(self) -> (
                &$lifetime_a $state<$meta::Driver>,
                &$lifetime_a [<$obj State>]<$meta::State>,
            ) {
                let old = self.state.[<get_old_ $obj:lower _state>](self.[<$obj:lower>]);

                // SAFETY: The old state is guaranteed to be present in `state` via our type
                // invariants.
                (self.state, unsafe { old.unwrap_unchecked() })
            }

            #[doc = concat!("Exchange this token for both the [`", stringify!($state),
                            "`] and the new [`", stringify!($obj), "State`].")]
            pub fn take_state_new_state(self) -> (
                &$lifetime_a $state<$meta::Driver>,
                $new_state,
            ) {
                let new = self.state.[<get_new_ $obj:lower _state>](self.[<$obj:lower>]);

                // SAFETY:
                // - The new state is guaranteed to be present in `state` via our type
                //   invariants.
                // - The new state is guaranteed to have no mutators taken out via our type
                //   invariants.
                (self.state, unsafe { new.unwrap_unchecked() })
            }
        }

        #[doc = concat!("Exchange this token for the [`", stringify!($state), "`].")]
        pub fn take_state(self) -> &$lifetime_a $state<$meta::Driver> {
            self.state
        }
    };
}

pub(crate) use impl_atomic_state_token_ops;

// Invariant in `'id`: overlapping transactions cannot shorten their lifetimes to a common one.
// Only `with_commit_scope` creates these brands. Its higher-ranked callback gives each invocation
// a fresh identity that cannot escape through the callback's lifetime-independent return type.
struct CommitScope<'id, T>(PhantomData<fn(&'id T) -> &'id T>);

impl<T> Copy for CommitScope<'_, T> {}

impl<T> Clone for CommitScope<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

fn with_commit_scope<T, R>(f: impl for<'id> FnOnce(CommitScope<'id, T>) -> R) -> R {
    f(CommitScope(PhantomData))
}

/// A token proving that no modesets for a commit have completed.
///
/// This token is proof that no commits have yet completed, and is provided as an argument to
/// [`KmsDriver::atomic_commit_tail`]. This may be used with
/// [`AtomicCommitTail::commit_modeset_disables`].
pub struct ModesetsReadyToken<'a, T: KmsDriver>(CommitScope<'a, T>);

/// A token proving that modeset disables for a commit have completed.
///
/// This token is proof that an implementor's [`KmsDriver::atomic_commit_tail`] phase has finished
/// committing any operations which disable mode objects. It is returned by
/// [`AtomicCommitTail::commit_modeset_disables`], and can be used with
/// [`AtomicCommitTail::commit_modeset_enables`] to acquire a [`EnablesCommittedToken`].
pub struct DisablesCommittedToken<'a, T: KmsDriver>(CommitScope<'a, T>);

/// A token proving that modeset enables for a commit have completed.
///
/// This token is proof that an implementor's [`KmsDriver::atomic_commit_tail`] phase has finished
/// committing any operations which enable mode objects. It is returned by
/// [`AtomicCommitTail::commit_modeset_enables`].
pub struct EnablesCommittedToken<'a, T: KmsDriver>(CommitScope<'a, T>);

/// A token proving that no plane updates for a commit have completed.
///
/// This token is proof that no plane updates have yet been completed within an implementor's
/// [`KmsDriver::atomic_commit_tail`] implementation, and that we are ready to begin updating planes. It
/// is provided as an argument to [`KmsDriver::atomic_commit_tail`].
pub struct PlaneUpdatesReadyToken<'a, T: KmsDriver>(CommitScope<'a, T>);

/// A token proving that all plane updates for a commit have completed.
///
/// This token is proof that all plane updates within an implementor's [`KmsDriver::atomic_commit_tail`]
/// implementation have completed. It is returned by [`AtomicCommitTail::commit_planes`].
pub struct PlaneUpdatesCommittedToken<'a, T: KmsDriver>(CommitScope<'a, T>);

/// An [`AtomicState`] interface that allows a driver to control the [`atomic_commit_tail`]
/// callback.
///
/// This object is provided as an argument to [`KmsDriver::atomic_commit_tail`], and represents an atomic
/// state within the commit tail phase which is still in the process of being committed to hardware.
/// It may be used to control the order in which the commit process happens.
///
/// # Invariants
///
/// Same as [`AtomicState`]. The invariant brand and the state borrow are scoped to one invocation
/// of the commit-tail callback; only tokens carrying that brand and driver type are accepted.
///
/// [`atomic_commit_tail`]: srctree/include/drm/drm_modeset_helper_vtables.h
pub struct AtomicCommitTail<'a, T: KmsDriver>(&'a AtomicState<T>, CommitScope<'a, T>);

impl<'a, T: KmsDriver> AtomicCommitTail<'a, T> {
    /// Commit modesets which would disable outputs.
    ///
    /// This function commits any modesets which would shut down outputs, along with preparing them
    /// for a new mode (if needed).
    ///
    /// Since it is physically impossible to disable an output multiple times, and since it is
    /// logically unsound to disable an output within an atomic commit after the output was enabled
    /// in the same commit - this function requires a [`ModesetsReadyToken`] to consume and returns
    /// a [`DisablesCommittedToken`].
    ///
    /// If compatibility with legacy CRTC helpers is desired, this
    /// should be called before [`commit_planes`] which is what the default commit function does.
    /// But drivers with different needs can group the modeset commits tgether and do the plane
    /// commits at the end. This is useful for drivers doing runtime PM since then plane updates
    /// only happen when the CRTC is actually enabled.
    ///
    /// [`commit_planes`]: AtomicCommitTail::commit_planes
    #[inline]
    #[must_use]
    pub fn commit_modeset_disables(
        &mut self,
        token: ModesetsReadyToken<'a, T>,
    ) -> DisablesCommittedToken<'a, T> {
        // SAFETY: Both `as_raw()` calls are guaranteed to return valid pointers
        unsafe {
            bindings::drm_atomic_helper_commit_modeset_disables(
                self.0.drm_dev().as_raw(),
                self.0.as_raw(),
            )
        }

        DisablesCommittedToken(token.0)
    }

    /// Commit all plane updates.
    ///
    /// This function performs all plane updates for the given [`AtomicCommitTail`]. Since it is
    /// logically unsound to perform the same plane update more then once in a given atomic commit,
    /// this function requires a [`PlaneUpdatesReadyToken`] to consume and returns a
    /// [`PlaneUpdatesCommittedToken`] to prove that plane updates for the state have completed.
    #[inline]
    #[must_use]
    pub fn commit_planes(
        &mut self,
        token: PlaneUpdatesReadyToken<'a, T>,
        flags: PlaneCommitFlags,
    ) -> PlaneUpdatesCommittedToken<'a, T> {
        // SAFETY: Both `as_raw()` calls are guaranteed to return valid pointers
        unsafe {
            bindings::drm_atomic_helper_commit_planes(
                self.0.drm_dev().as_raw(),
                self.0.as_raw(),
                flags.into(),
            )
        }

        PlaneUpdatesCommittedToken(token.0)
    }

    /// Commit modesets which would enable outputs.
    ///
    /// This function commits any modesets in the given [`AtomicCommitTail`] which would enable
    /// outputs, along with preparing them for their new modes (if needed).
    ///
    /// Since it is logically unsound to enable an output before any disabling modesets within the
    /// same atomic commit have been performed, and physically impossible to enable the same output
    /// multiple times - this function requires a [`DisablesCommittedToken`] to consume and returns
    /// a [`EnablesCommittedToken`] which may be used as proof that all modesets in the state have
    /// been completed.
    #[inline]
    #[must_use]
    pub fn commit_modeset_enables(
        &mut self,
        token: DisablesCommittedToken<'a, T>,
    ) -> EnablesCommittedToken<'a, T> {
        // SAFETY: Both `as_raw()` calls are guaranteed to return valid pointers
        unsafe {
            bindings::drm_atomic_helper_commit_modeset_enables(
                self.0.drm_dev().as_raw(),
                self.0.as_raw(),
            )
        }

        EnablesCommittedToken(token.0)
    }

    /// Fake vblank events if needed.
    ///
    /// Note that this is still relevant to drivers which don't implement [`VblankSupport`] for any
    /// of their CRTCs.
    ///
    /// TODO: more doc
    ///
    /// [`VblankSupport`]: super::vblank::VblankSupport
    pub fn fake_vblank(&mut self) {
        // SAFETY: `as_raw()` is guaranteed to always return a valid pointer
        unsafe { bindings::drm_atomic_helper_fake_vblank(self.0.as_raw()) }
    }

    /// Signal completion of the hardware commit step.
    ///
    /// The software states have already been published before entering the commit worker. This
    /// marks hardware programming as completed. Since this step can only happen after plane updates and
    /// modesets within an [`AtomicCommitTail`] have been completed, it requires both a
    /// [`EnablesCommittedToken`] and a [`PlaneUpdatesCommittedToken`] to consume. After this
    /// function is called, another commit may replace and free the published new states. As such,
    /// this function consumes the [`AtomicCommitTail`] object and returns a
    /// [`CommittedAtomicState`] accessor for performing post-hw commit tasks.
    pub fn commit_hw_done(
        self,
        _modeset_token: EnablesCommittedToken<'a, T>,
        _plane_updates_token: PlaneUpdatesCommittedToken<'a, T>,
    ) -> CommittedAtomicState<'a, T> {
        // SAFETY: we consume the `AtomicCommitTail` object, making it impossible for the user to
        // mutate the state after this function has been called - which upholds the safety
        // requirements of the C API allowing us to safely call this function
        unsafe { bindings::drm_atomic_helper_commit_hw_done(self.0.as_raw()) };

        CommittedAtomicState {
            state: self.0,
            _scope: self.1,
            flip_done_waited: Cell::new(false),
        }
    }
}

// The actual raw C callback for custom atomic commit tail implementations
pub(crate) unsafe extern "C" fn commit_tail_callback<T: KmsDriver>(
    state: *mut bindings::drm_atomic_commit,
) {
    with_commit_scope(|scope| {
        // SAFETY: DRM supplies a valid commit for this driver's callback. The shared state
        // borrow is confined to the fresh invariant scope, and the callback finishes before
        // returning to C. No branded tail or phase token can escape this closure.
        let state = unsafe { AtomicState::from_raw(state.cast_const()) };

        T::atomic_commit_tail(
            AtomicCommitTail(state, scope),
            ModesetsReadyToken(scope),
            PlaneUpdatesReadyToken(scope),
        );
    });
}

/// An [`AtomicState`] which was just committed with [`AtomicCommitTail::commit_hw_done`].
///
/// Hardware programming has completed, but old scanout resources may still be in use. This
/// accessor permits waiting for flip completion and releases old plane resources when dropped.
/// It exposes neither the published new states nor mutable private payloads.
///
/// Commit workers do not hold modesetting locks. After hardware completion another commit may
/// replace and destroy the new states, regardless of references held to the enclosing transaction.
///
/// # Invariants
///
/// It may be assumed that [`drm_atomic_helper_commit_hw_done`] has been called as long as this type
/// exists. The invariant transaction scope is retained through cleanup, so a completed state
/// cannot be shortened to substitute for another callback's completion proof.
///
/// [`atomic_commit_tail`]: KmsDriver::atomic_commit_tail
/// [`drm_atomic_helper_commit_hw_done`]: srctree/include/drm/drm_atomic_helper.h
pub struct CommittedAtomicState<'a, T: KmsDriver> {
    state: &'a AtomicState<T>,
    _scope: CommitScope<'a, T>,
    flip_done_waited: Cell<bool>,
}

impl<'a, T: KmsDriver> CommittedAtomicState<'a, T> {
    /// Wait for page flips on this state to complete.
    ///
    /// Dropping the accessor performs this wait if it has not already been done. As with the
    /// C helper, hardware failure may time out; completing the wait is not proof of pixel validity.
    pub fn wait_for_flip_done(&self) {
        if self.flip_done_waited.get() {
            return;
        }
        // SAFETY: `drm_atomic_helper_commit_hw_done` has been called via our invariants
        unsafe {
            bindings::drm_atomic_helper_wait_for_flip_done(
                self.state.drm_dev().as_raw(),
                self.state.as_raw(),
            )
        }
        self.flip_done_waited.set(true);
    }
}

impl<'a, T: KmsDriver> Drop for CommittedAtomicState<'a, T> {
    fn drop(&mut self) {
        // Hardware programming completion does not mean the old scanout buffer is no longer
        // in use. Preserve the helper's flip-before-cleanup ordering even if the driver omits
        // an explicit wait before returning its completed-state accessor.
        self.wait_for_flip_done();
        // SAFETY:
        // * This interface represents the last atomic state accessor which could be affected as a
        //   result of resources from an atomic commit being cleaned up.
        unsafe {
            bindings::drm_atomic_helper_cleanup_planes(
                self.state.drm_dev().as_raw(),
                self.state.as_raw(),
            )
        }
    }
}

/// An enumator representing a single flag in [`PlaneCommitFlags`].
///
/// This is a non-exhaustive list, as the C side could add more later.
#[derive(Copy, Clone, PartialEq, Eq)]
#[repr(u32)]
#[non_exhaustive]
pub enum PlaneCommitFlag {
    /// Don't notify applications of plane updates for newly-disabled planes. Drivers are encouraged
    /// to set this flag by default, as otherwise they need to ignore plane updates for disabled
    /// planes by hand.
    ActiveOnly = (1 << 0),
    /// Tell the DRM core that the display hardware requires that a [`Crtc`]'s planes must be
    /// disabled when the [`Crtc`] is disabled. When not specified,
    /// [`AtomicCommitTail::commit_planes`] will skip the atomic disable callbacks for a plane if
    /// the [`Crtc`] in the old [`PlaneState`] needs a modesetting operation. It is still up to the
    /// driver to disable said planes in their [`DriverCrtc::atomic_disable`] callback.
    NoDisableAfterModeset = (1 << 1),
}

impl BitOr for PlaneCommitFlag {
    type Output = PlaneCommitFlags;

    fn bitor(self, rhs: Self) -> Self::Output {
        PlaneCommitFlags(self as u32 | rhs as u32)
    }
}

impl BitOr<PlaneCommitFlags> for PlaneCommitFlag {
    type Output = PlaneCommitFlags;

    fn bitor(self, rhs: PlaneCommitFlags) -> Self::Output {
        PlaneCommitFlags(self as u32 | rhs.0)
    }
}

/// A bitmask for controlling the behavior of [`AtomicCommitTail::commit_planes`].
///
/// This corresponds to the `DRM_PLANE_COMMIT_*` flags on the C side. Note that this bitmask does
/// not discard unknown values in order to ensure that adding new flags on the C side of things does
/// not break anything in the future.
#[derive(Copy, Clone, Default, PartialEq, Eq)]
pub struct PlaneCommitFlags(u32);

impl From<PlaneCommitFlag> for PlaneCommitFlags {
    fn from(value: PlaneCommitFlag) -> Self {
        Self(value as u32)
    }
}

impl From<PlaneCommitFlags> for u32 {
    fn from(value: PlaneCommitFlags) -> Self {
        value.0
    }
}

impl BitOr for PlaneCommitFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for PlaneCommitFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        *self = *self | rhs
    }
}

impl BitAnd for PlaneCommitFlags {
    type Output = PlaneCommitFlags;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl BitAndAssign for PlaneCommitFlags {
    fn bitand_assign(&mut self, rhs: Self) {
        *self = *self & rhs
    }
}

impl BitOr<PlaneCommitFlag> for PlaneCommitFlags {
    type Output = Self;

    fn bitor(self, rhs: PlaneCommitFlag) -> Self::Output {
        self | Self::from(rhs)
    }
}

impl BitOrAssign<PlaneCommitFlag> for PlaneCommitFlags {
    fn bitor_assign(&mut self, rhs: PlaneCommitFlag) {
        *self = *self | rhs
    }
}

impl BitAnd<PlaneCommitFlag> for PlaneCommitFlags {
    type Output = PlaneCommitFlags;

    fn bitand(self, rhs: PlaneCommitFlag) -> Self::Output {
        self & Self::from(rhs)
    }
}

impl BitAndAssign<PlaneCommitFlag> for PlaneCommitFlags {
    fn bitand_assign(&mut self, rhs: PlaneCommitFlag) {
        *self = *self & rhs
    }
}

impl PlaneCommitFlags {
    /// Create a new bitmask.
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if the bitmask has the given commit flag set.
    pub fn has(&self, flag: PlaneCommitFlag) -> bool {
        *self & flag == flag.into()
    }
}
