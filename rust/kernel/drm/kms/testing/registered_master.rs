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
