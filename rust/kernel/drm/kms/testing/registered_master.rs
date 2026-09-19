// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Master-file ownership while a test retains device registration.

use super::{
    client::Client,
    masters::install_master,
    KmsDriver, //
};
use crate::{
    drm::{
        device::{
            Device,
            Registered, //
        },
        file::File,
        kms::{
            connector::{
                AsRawConnector,
                Connector, //
            },
            crtc::{
                AsRawCrtc,
                Crtc, //
            }, //
        }, //
    },
    prelude::*,
    sync::aref::ARef,
    types::ScopeGuard, //
};

/// Internal master file for a registered test display, without a userspace descriptor.
///
/// The registration borrow keeps native file close and its master-drop callback ahead of
/// unplug. Construction installs a test master only when no other master is current;
/// competing ownership returns `EBUSY`. The driver's real master callbacks run on creation
/// and close. A successful constructor is not continuing display authority.
/// Use ordinary current-master guards for each operation. No client callbacks are registered.
/// Call creation and destruction outside native master, object-ID and modeset locks.
pub struct RegisteredMasterFile<'a, T: KmsDriver> {
    client: Client<T>,
    registered: &'a Device<T, Registered>,
}

impl<'a, T: KmsDriver> RegisteredMasterFile<'a, T> {
    /// Open an internal file and install its independent file/device master references.
    pub fn new(device: &'a Device<T, Registered>) -> Result<Self> {
        let client = Client::new_registered(device)?;
        install_master(&client)?;
        Ok(Self {
            client,
            registered: device,
        })
    }

    /// Borrow the file while its registration and native ownership remain retained.
    pub fn file(&self) -> &File<T::File> {
        self.client.file()
    }

    /// Open a non-master file associated with this registered device's current owner.
    ///
    /// The peer retains its own file lifetime and may outlive this master file, but
    /// not the registered device. Issuance still has to check the current owner.
    pub fn associated_file(&self) -> Result<RegisteredAssociatedFile<'a, T>> {
        let client = Client::new_registered(self.registered)?;
        let identity = self.file().associated_master().ok_or(EINVAL)?;
        {
            let access = identity.lock_current().ok_or(EACCES)?;
            if !access.is_master_file(self.file()) {
                return Err(EACCES);
            }
            let peer = client.file().as_raw();
            let owner = self.file().as_raw();
            // SAFETY: The peer is initialized without a master association. The
            // current-owner guard stabilizes the native association while it is copied.
            unsafe {
                bindings::spin_lock(&raw mut (*peer).master_lookup_lock);
                (*peer).master = bindings::drm_master_get((*owner).master);
                bindings::spin_unlock(&raw mut (*peer).master_lookup_lock);
            }
        }
        Ok(RegisteredAssociatedFile { client, _registered: self.registered })
    }

    /// Borrow the sole static CRTC, rejecting a different test topology.
    pub fn crtc(&self) -> Result<&Crtc<T::Crtc>> {
        // SAFETY: Registration completed static CRTC creation and excludes teardown. Rust
        // construction enforces the nominated CRTC type for every object on this device.
        unsafe {
            let config = &(*self.registered.as_raw()).mode_config;
            if config.num_crtc != 1 {
                return Err(EINVAL);
            }
            let raw = crate::container_of!(config.crtc_list.next, bindings::drm_crtc, head);
            Ok(Crtc::from_raw(raw))
        }
    }

    /// Borrow a numbered CRTC in the registered device's static topology.
    /// Registration retains the completed list and excludes object destruction.
    pub fn crtc_at(&self, index: usize) -> Result<&Crtc<T::Crtc>> {
        // SAFETY: Registration retains every static CRTC. Bounds are checked before walking
        // the list, and Rust construction enforces the driver's nominated concrete type.
        unsafe {
            let config = &(*self.registered.as_raw()).mode_config;
            if index >= config.num_crtc as usize {
                return Err(EINVAL);
            }
            let mut entry = config.crtc_list.next;
            for _ in 0..index {
                entry = (*entry).next;
            }
            let raw = crate::container_of!(entry, bindings::drm_crtc, head);
            Ok(Crtc::from_raw(raw))
        }
    }

    /// Retain a connector at the current native iteration position.
    /// The owned reference survives list changes; the index is not a persistent identity.
    pub fn connector_at(&self, index: usize) -> Result<ARef<Connector<T::Connector>>> {
        let mut iterator = bindings::drm_connector_list_iter::default();
        // SAFETY: Registration retains the initialized connector list and its device.
        unsafe { bindings::drm_connector_list_iter_begin(self.registered.as_raw(), &mut iterator) };
        let mut iterator = ScopeGuard::new_with_data(iterator, |mut iterator| {
            // SAFETY: End the initialized iterator on every return path.
            unsafe { bindings::drm_connector_list_iter_end(&mut iterator) };
        });
        let mut position = 0;
        loop {
            // SAFETY: The iterator retains its current connector until advancement or end.
            let raw = unsafe { bindings::drm_connector_list_iter_next(&mut *iterator) };
            if raw.is_null() {
                return Err(EINVAL);
            }
            if position == index {
                // SAFETY: Acquire independent ownership before ending native iteration.
                return Ok(unsafe { Connector::<T::Connector>::from_raw(raw) }.into());
            }
            position += 1;
        }
    }

    /// Retain the sole connector, with native iteration protecting its lookup lifetime.
    ///
    /// The reference does not prove continuing registration or lease membership.
    pub fn connector(&self) -> Result<ARef<Connector<T::Connector>>> {
        let mut iterator = bindings::drm_connector_list_iter::default();
        // SAFETY: Registration keeps the initialized device and connector list live.
        unsafe { bindings::drm_connector_list_iter_begin(self.registered.as_raw(), &mut iterator) };
        let mut iterator = ScopeGuard::new_with_data(iterator, |mut iterator| {
            // SAFETY: End the initialized iterator after all temporary native borrows end.
            unsafe { bindings::drm_connector_list_iter_end(&mut iterator) };
        });
        // SAFETY: The live iterator retains each returned connector until advancement or end.
        let first = unsafe { bindings::drm_connector_list_iter_next(&mut *iterator) };
        if first.is_null() {
            return Err(EINVAL);
        }
        // SAFETY: The iterator retains this driver's connector. Rust constructors enforce
        // its nominated type; acquire independent connector/device ownership before advancing.
        let connector: ARef<_> = unsafe { Connector::<T::Connector>::from_raw(first) }.into();
        // SAFETY: Advancing the initialized iterator releases only its own previous reference.
        let another = !unsafe { bindings::drm_connector_list_iter_next(&mut *iterator) }.is_null();
        drop(iterator);
        if another {
            return Err(EINVAL);
        }
        Ok(connector)
    }
}

/// Non-master peer of a registered test display's current owner.
pub struct RegisteredAssociatedFile<'a, T: KmsDriver> {
    client: Client<T>,
    _registered: &'a Device<T, Registered>,
}

impl<T: KmsDriver> RegisteredAssociatedFile<'_, T> {
    /// Borrow the peer without extending its close lifetime.
    pub fn file(&self) -> &File<T::File> {
        self.client.file()
    }
}
