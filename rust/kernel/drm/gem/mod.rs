// SPDX-License-Identifier: GPL-2.0 OR MIT

//! DRM GEM API
//!
//! C header: [`include/drm/drm_gem.h`](srctree/include/drm/drm_gem.h)

use crate::{
    bindings,
    drm::{
        self,
        device::{
            DeviceContext,
            Normal, //
        },
        driver::{
            AllocImpl,
            AllocOps, //
        },
    },
    error::to_result,
    prelude::*,
    sync::aref::ARef,
    types::Opaque,
};
use core::{
    marker::PhantomData,
    ops::Deref,
    ptr::NonNull, //
};

#[cfg(CONFIG_RUST_DRM_GEM_SHMEM_HELPER)]
pub mod shmem;

/// An owned GEM reference retaining the object's DRM device until after object release.
///
/// Native GEM references do not themselves retain the device. Keep this handle for detached
/// Rust work; a driver storing it in device-owned state must break that ownership cycle during
/// shutdown. Allocation lifetime does not establish permission to read or write buffer contents.
pub struct ObjectRef<O: IntoGEMObject + AllocImpl> {
    object: NonNull<O>,
    _device: ARef<drm::Device<O::Driver>>,
}

// SAFETY: The owned object and device are retained, and the object permits thread transfer.
unsafe impl<O: IntoGEMObject + AllocImpl + Send + Sync> Send for ObjectRef<O> {}
// SAFETY: Shared access exposes only the object's thread-safe borrowed interface.
unsafe impl<O: IntoGEMObject + AllocImpl + Sync> Sync for ObjectRef<O> {}

impl<O: IntoGEMObject + AllocImpl> ObjectRef<O> {
    /// Adopt one native GEM reference while acquiring a reference to its device.
    ///
    /// # Safety
    ///
    /// `object` must point to an initialized `O` with one owned native reference. Its device
    /// must be live and have driver `O::Driver`. Ownership transfers to the returned handle.
    pub(crate) unsafe fn from_native(object: NonNull<O>) -> Self {
        // SAFETY: The caller supplies a typed live object and a live matching device.
        let device = unsafe { drm::Device::<O::Driver>::from_raw((*object.as_ref().as_raw()).dev) };
        Self {
            object,
            _device: device.into(),
        }
    }

    /// Transfer the native GEM reference, releasing this handle's device reference.
    ///
    /// # Safety
    ///
    /// The caller must independently retain the device until the transferred reference is
    /// released. The recipient must release exactly one reference with the native GEM API.
    pub(crate) unsafe fn into_native(self) -> *mut bindings::drm_gem_object {
        let mut this = core::mem::ManuallyDrop::new(self);
        let raw = this.as_raw();
        // SAFETY: ManuallyDrop suppresses our destructor. Transfer the GEM reference and drop
        // exactly the device field; the caller supplies its independent device lifetime.
        unsafe { core::ptr::drop_in_place(&raw mut this._device) };
        raw
    }
}

impl<O: IntoGEMObject + AllocImpl> From<&O> for ObjectRef<O> {
    fn from(object: &O) -> Self {
        // SAFETY: A valid borrowed object proves its native reference and device are live.
        unsafe { bindings::drm_gem_object_get(object.as_raw()) };
        // SAFETY: Transfer the reference just acquired, with the device live through the borrow.
        unsafe { Self::from_native(object.into()) }
    }
}

impl<O: IntoGEMObject + AllocImpl> Deref for ObjectRef<O> {
    type Target = O;

    fn deref(&self) -> &O {
        // SAFETY: Both the object and its device are retained by this handle.
        unsafe { self.object.as_ref() }
    }
}

impl<O: IntoGEMObject + AllocImpl> Clone for ObjectRef<O> {
    fn clone(&self) -> Self {
        Self::from(&**self)
    }
}

impl<O: IntoGEMObject + AllocImpl> Drop for ObjectRef<O> {
    fn drop(&mut self) {
        // SAFETY: Release our native reference while the device field is still live.
        unsafe { bindings::drm_gem_object_put(self.as_raw()) };
    }
}

/// A type alias for retrieving a [`Driver`]s [`DriverFile`] implementation from its
/// [`DriverObject`] implementation.
///
/// [`Driver`]: drm::Driver
/// [`DriverFile`]: drm::file::DriverFile
pub type DriverFile<T> = drm::File<<<T as DriverObject>::Driver as drm::Driver>::File>;

/// A type alias for retrieving the current [`AllocImpl`] for a given [`DriverObject`].
///
/// [`Driver`]: drm::Driver
pub type DriverAllocImpl<T> = <<T as DriverObject>::Driver as drm::Driver>::Object;

/// GEM object functions, which must be implemented by drivers.
pub trait DriverObject: Sync + Send + Sized + 'static {
    /// Parent `Driver` for this object.
    type Driver: drm::Driver;

    /// The data type to use for passing arguments to [`DriverObject::new`].
    type Args;

    /// Create a new driver data object for a GEM object of a given size.
    fn new(
        dev: &drm::Device<Self::Driver>,
        size: usize,
        args: Self::Args,
    ) -> impl PinInit<Self, Error>;

    /// Open a new handle to an existing object, associated with a File.
    fn open(_obj: &DriverAllocImpl<Self>, _file: &DriverFile<Self>) -> Result {
        Ok(())
    }

    /// Close a handle to an existing object, associated with a File.
    fn close(_obj: &DriverAllocImpl<Self>, _file: &DriverFile<Self>) {}
}

/// Trait that represents a GEM object subtype
pub trait IntoGEMObject: Sized + super::private::Sealed {
    /// Returns a reference to the raw `drm_gem_object` structure, which must be valid as long as
    /// this owning object is valid.
    fn as_raw(&self) -> *mut bindings::drm_gem_object;

    /// Converts a pointer to a `struct drm_gem_object` into a reference to `Self`.
    ///
    /// # Safety
    ///
    /// - `self_ptr` must be a valid pointer to the `struct drm_gem_object` embedded in a
    ///   valid instance of `Self`.
    /// - The caller promises that holding the immutable reference returned by this function does
    ///   not violate rust's data aliasing rules and remains valid throughout the lifetime of `'a`.
    unsafe fn from_raw<'a>(self_ptr: *mut bindings::drm_gem_object) -> &'a Self;
}

extern "C" fn open_callback<T: DriverObject>(
    raw_obj: *mut bindings::drm_gem_object,
    raw_file: *mut bindings::drm_file,
) -> core::ffi::c_int {
    // SAFETY: `open_callback` is only ever called with a valid pointer to a `struct drm_file`.
    let file = unsafe { DriverFile::<T>::from_raw(raw_file) };

    // SAFETY:
    // * `open_callback` is specified in the AllocOps structure for `DriverObject`, ensuring that
    //   `raw_obj` is contained within a `DriverAllocImpl<T>`
    // * It is only possible for `open_callback` to be called after device registration, ensuring
    //   that the object's device is in the `Registered` state.
    let obj: &DriverAllocImpl<T> = unsafe { IntoGEMObject::from_raw(raw_obj) };

    match T::open(obj, file) {
        Err(e) => e.to_errno(),
        Ok(()) => 0,
    }
}

extern "C" fn close_callback<T: DriverObject>(
    raw_obj: *mut bindings::drm_gem_object,
    raw_file: *mut bindings::drm_file,
) {
    // SAFETY: `open_callback` is only ever called with a valid pointer to a `struct drm_file`.
    let file = unsafe { DriverFile::<T>::from_raw(raw_file) };

    // SAFETY: `close_callback` is specified in the AllocOps structure for `Object<T>`, ensuring
    // that `raw_obj` is indeed contained within a `Object<T>`.
    let obj: &DriverAllocImpl<T> = unsafe { IntoGEMObject::from_raw(raw_obj) };

    T::close(obj, file);
}

impl<T: DriverObject, Ctx: DeviceContext> IntoGEMObject for Object<T, Ctx> {
    fn as_raw(&self) -> *mut bindings::drm_gem_object {
        self.obj.get()
    }

    unsafe fn from_raw<'a>(self_ptr: *mut bindings::drm_gem_object) -> &'a Self {
        // SAFETY: `obj` is guaranteed to be in an `Object<T>` via the safety contract of this
        // function
        unsafe { &*crate::container_of!(Opaque::cast_from(self_ptr), Object<T, Ctx>, obj) }
    }
}

/// Base operations shared by all GEM object classes
pub trait BaseObject: IntoGEMObject {
    /// Returns the size of the object in bytes.
    fn size(&self) -> usize {
        // SAFETY: `self.as_raw()` is guaranteed to be a pointer to a valid `struct drm_gem_object`.
        unsafe { (*self.as_raw()).size }
    }

    /// Creates a new handle for the object associated with a given `File`
    /// (or returns an existing one).
    fn create_handle<D, F>(&self, file: &drm::File<F>) -> Result<u32>
    where
        Self: AllocImpl<Driver = D>,
        D: drm::Driver<Object = Self, File = F>,
        F: drm::file::DriverFile<Driver = D>,
    {
        // The associated-type bounds prove a common driver type; separately reject another
        // instance of that driver before passing the pair to the C API.
        // SAFETY: `self.as_raw()` is a valid GEM object by the trait invariant.
        if unsafe { (*self.as_raw()).dev } != file.device_raw() {
            return Err(EINVAL);
        }

        let mut handle: u32 = 0;
        // SAFETY: The arguments are all valid per the type invariants.
        to_result(unsafe {
            bindings::drm_gem_handle_create(file.as_raw().cast(), self.as_raw(), &mut handle)
        })?;
        Ok(handle)
    }

    /// Looks up an object by its handle for a given `File`.
    fn lookup_handle<D, F>(file: &drm::File<F>, handle: u32) -> Result<ObjectRef<Self>>
    where
        Self: AllocImpl<Driver = D>,
        D: drm::Driver<Object = Self, File = F>,
        F: drm::file::DriverFile<Driver = D>,
    {
        // SAFETY: The arguments are all valid per the type invariants.
        let ptr = unsafe { bindings::drm_gem_object_lookup(file.as_raw().cast(), handle) };
        if ptr.is_null() {
            return Err(ENOENT);
        }

        // SAFETY:
        // - A `drm::Driver` can only have a single `File` implementation.
        // - `file` uses the same `drm::Driver` as `Self`.
        // - Therefore, we're guaranteed that `ptr` must be a gem object embedded within `Self`.
        // - And we check if the pointer is null befoe calling from_raw(), ensuring that `ptr` is a
        //   valid pointer to an initialized `Self`.
        let obj = unsafe { Self::from_raw(ptr) };

        // SAFETY:
        // - We take ownership of the reference of `drm_gem_object_lookup()`.
        // - Our `NonNull` comes from an immutable reference, thus ensuring it is a valid pointer to
        //   `Self`.
        Ok(unsafe { ObjectRef::from_native(obj.into()) })
    }

    /// Creates an mmap offset to map the object from userspace.
    fn create_mmap_offset(&self) -> Result<u64> {
        // SAFETY: The arguments are valid per the type invariant.
        to_result(unsafe { bindings::drm_gem_create_mmap_offset(self.as_raw()) })?;

        // SAFETY: The arguments are valid per the type invariant.
        Ok(unsafe { bindings::drm_vma_node_offset_addr(&raw mut (*self.as_raw()).vma_node) })
    }
}

impl<T: IntoGEMObject> BaseObject for T {}

/// Crate-private base operations shared by all GEM object classes.
#[cfg_attr(not(CONFIG_RUST_DRM_GEM_SHMEM_HELPER), expect(unused))]
pub(crate) trait BaseObjectPrivate: IntoGEMObject {
    /// Return a pointer to this object's dma_resv.
    fn raw_dma_resv(&self) -> *mut bindings::dma_resv {
        // SAFETY: `self.as_raw()` always returns a valid pointer to the base DRM GEM object.
        unsafe { (*self.as_raw()).resv }
    }
}

impl<T: IntoGEMObject> BaseObjectPrivate for T {}

/// A base GEM object.
///
/// # Invariants
///
/// * `self.obj` is a valid instance of a `struct drm_gem_object`.
/// * Any type invariants of `Ctx` apply to the parent DRM device for this GEM object.
#[repr(C)]
#[pin_data]
pub struct Object<T: DriverObject + Send + Sync, Ctx: DeviceContext = Normal> {
    obj: Opaque<bindings::drm_gem_object>,
    #[pin]
    data: T,
    _ctx: PhantomData<Ctx>,
}

impl<T: DriverObject, Ctx: DeviceContext> Object<T, Ctx> {
    const OBJECT_FUNCS: bindings::drm_gem_object_funcs = bindings::drm_gem_object_funcs {
        free: Some(Self::free_callback),
        open: Some(open_callback::<T>),
        close: Some(close_callback::<T>),
        print_info: None,
        export: None,
        pin: None,
        unpin: None,
        get_sg_table: None,
        vmap: None,
        vunmap: None,
        mmap: None,
        status: None,
        vm_ops: core::ptr::null_mut(),
        evict: None,
        rss: None,
    };

    /// Returns the `Device` that owns this GEM object.
    pub fn dev(&self) -> &drm::Device<T::Driver, Ctx> {
        // SAFETY:
        // - `struct drm_gem_object.dev` is initialized and valid for as long as the GEM
        //   object lives.
        // - The device we used for creating the gem object is passed as &drm::Device<T::Driver> to
        //   Object::<T>::new(), so we know that `T::Driver` is the right generic parameter to use
        //   here.
        // - Any type invariants of `Ctx` are upheld by using the same `Ctx` for the `Device` we
        //   return.
        unsafe { drm::Device::from_raw((*self.as_raw()).dev) }
    }

    fn as_raw(&self) -> *mut bindings::drm_gem_object {
        self.obj.get()
    }

    extern "C" fn free_callback(obj: *mut bindings::drm_gem_object) {
        let ptr: *mut Opaque<bindings::drm_gem_object> = obj.cast();

        // SAFETY: All of our objects are of type `Object<T>`.
        let this = unsafe { crate::container_of!(ptr, Self, obj) };

        // SAFETY: The C code only ever calls this callback with a valid pointer to a `struct
        // drm_gem_object`.
        unsafe { bindings::drm_gem_object_release(obj) };

        // SAFETY: All of our objects are allocated via `KBox`, and we're in the
        // free callback which guarantees this object has zero remaining references,
        // so we can drop it.
        let _ = unsafe { KBox::from_raw(this) };
    }
}

impl<T: DriverObject> Object<T> {
    /// Create a new GEM object.
    pub fn new(dev: &drm::Device<T::Driver>, size: usize, args: T::Args) -> Result<ObjectRef<Self>>
    where
        T::Driver: drm::Driver<Object = Self>,
    {
        let obj: Pin<KBox<Self>> = KBox::pin_init(
            try_pin_init!(Self {
                obj: Opaque::new(bindings::drm_gem_object::default()),
                data <- T::new(dev, size, args),
                _ctx: PhantomData,
            }),
            GFP_KERNEL,
        )?;

        // SAFETY: `obj.as_raw()` is guaranteed to be valid by the initialization above.
        unsafe { (*obj.as_raw()).funcs = &Self::OBJECT_FUNCS };

        // INVARIANT: `dev` and the GEM object are in the same state at the moment, and upgrading
        // the typestate in `dev` will not carry over to the GEM object.
        if let Err(err) =
            // SAFETY: The arguments are all valid per the type invariants.
            to_result(unsafe {
                bindings::drm_gem_object_init(dev.as_raw(), obj.obj.get(), size)
            })
        {
            // SAFETY: `drm_gem_object_init()` initializes the private GEM object state before
            // failing, so `drm_gem_private_object_fini()` is the matching cleanup.
            unsafe { bindings::drm_gem_private_object_fini(obj.obj.get()) };
            return Err(err);
        }

        // SAFETY: The owned reference never moves the object allocation.
        let ptr = KBox::into_raw(unsafe { Pin::into_inner_unchecked(obj) });

        // SAFETY: `ptr` comes from `KBox::into_raw` and hence can't be NULL.
        let ptr = unsafe { NonNull::new_unchecked(ptr) };

        // SAFETY: We take over the initial reference count from `drm_gem_object_init()`.
        Ok(unsafe { ObjectRef::from_native(ptr) })
    }
}

impl<T: DriverObject, Ctx: DeviceContext> super::private::Sealed for Object<T, Ctx> {}

impl<T: DriverObject, Ctx: DeviceContext> Deref for Object<T, Ctx> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T: DriverObject, Ctx: DeviceContext> AllocImpl for Object<T, Ctx> {
    type Driver = T::Driver;

    const ALLOC_OPS: AllocOps = AllocOps {
        gem_create_object: None,
        prime_handle_to_fd: None,
        prime_fd_to_handle: None,
        gem_prime_import: None,
        gem_prime_import_sg_table: None,
        dumb_create: None,
        dumb_map_offset: None,
        fbdev_probe: None,
    };
}

pub(super) const fn create_fops(owner: *mut bindings::module) -> bindings::file_operations {
    let mut fops: bindings::file_operations = pin_init::zeroed();

    fops.owner = owner;
    fops.open = Some(bindings::drm_open);
    fops.release = Some(bindings::drm_release);
    fops.unlocked_ioctl = Some(bindings::drm_ioctl);
    #[cfg(CONFIG_COMPAT)]
    {
        fops.compat_ioctl = Some(bindings::drm_compat_ioctl);
    }
    fops.poll = Some(bindings::drm_poll);
    fops.read = Some(bindings::drm_read);
    fops.llseek = Some(bindings::noop_llseek);
    fops.mmap = Some(bindings::drm_gem_mmap);
    fops.fop_flags = bindings::FOP_UNSIGNED_OFFSET;

    fops
}
