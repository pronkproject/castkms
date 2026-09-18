// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Safe callbacks and notifications for an HDMI CEC adapter on a DRM connector.

use super::{AsRawConnector, Connector, DriverConnector, UnregisteredConnector};
use crate::{bindings, device, error::to_result, prelude::*, sync::aref::ARef};

/// Maximum payload size of one CEC message.
pub const MAX_MESSAGE_SIZE: usize = bindings::CEC_MAX_MSG_SIZE as usize;

/// One CEC message copied out of the native callback.
#[derive(Clone, Copy)]
pub struct Message {
    bytes: [u8; MAX_MESSAGE_SIZE],
    len: u8,
}

impl Message {
    /// Construct a checked CEC message.
    pub fn new(bytes: &[u8]) -> Result<Self> {
        if bytes.is_empty() || bytes.len() > MAX_MESSAGE_SIZE {
            return Err(EINVAL);
        }
        let mut message = Self {
            bytes: [0; MAX_MESSAGE_SIZE],
            len: bytes.len() as u8,
        };
        message.bytes[..bytes.len()].copy_from_slice(bytes);
        Ok(message)
    }

    /// Return the initialized message bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    fn from_native(message: &bindings::cec_msg) -> Result<Self> {
        let len = message.len as usize;
        if len == 0 || len > MAX_MESSAGE_SIZE {
            return Err(EINVAL);
        }
        let mut result = Self {
            bytes: [0; MAX_MESSAGE_SIZE],
            len: len as u8,
        };
        result.bytes[..len].copy_from_slice(&message.msg[..len]);
        Ok(result)
    }

    fn to_native(self) -> bindings::cec_msg {
        let mut message = bindings::cec_msg::default();
        message.len = u32::from(self.len);
        message.msg[..self.len as usize].copy_from_slice(self.as_bytes());
        message
    }
}

/// Completion counters reported for one transmitted message.
#[derive(Clone, Copy, Default)]
pub struct TransmitResult {
    /// A `CEC_TX_STATUS_*` bit mask.
    pub status: u8,
    /// Arbitration-loss attempts.
    pub arbitration_lost: u8,
    /// NACKed attempts.
    pub nack: u8,
    /// Low-drive failures.
    pub low_drive: u8,
    /// Other failed attempts.
    pub error: u8,
}

/// Driver callbacks behind one registered CEC adapter.
pub trait DriverCec: DriverConnector {
    /// Initialize driver-private CEC state immediately before adapter registration.
    fn cec_init(_connector: &Connector<Self>) -> Result {
        Ok(())
    }

    /// Retire driver-private CEC state during managed device teardown.
    fn cec_uninit(_connector: &Connector<Self>) {}

    /// Enable or disable the adapter.
    fn cec_enable(connector: &Connector<Self>, enable: bool) -> Result;

    /// Add a logical address, or clear all addresses for `CEC_LOG_ADDR_INVALID`.
    fn cec_logical_address(connector: &Connector<Self>, address: u8) -> Result;

    /// Start one transmission. Completion is reported separately.
    fn cec_transmit(
        connector: &Connector<Self>,
        attempts: u8,
        signal_free_time: u32,
        message: Message,
    ) -> Result;
}

impl<T: DriverCec> Connector<T> {
    const CEC_OPS: bindings::drm_connector_hdmi_cec_funcs =
        bindings::drm_connector_hdmi_cec_funcs {
            init: Some(cec_init_callback::<T>),
            uninit: Some(cec_uninit_callback::<T>),
            enable: Some(cec_enable_callback::<T>),
            log_addr: Some(cec_logical_address_callback::<T>),
            transmit: Some(cec_transmit_callback::<T>),
        };

    /// Deliver a received CEC message to the registered adapter.
    pub fn cec_received(&self, message: Message) {
        let mut message = message.to_native();
        // SAFETY: Successful registration retains the helper data until managed
        // connector teardown, and the helper copies the complete message.
        unsafe { bindings::drm_connector_hdmi_cec_received_msg(self.as_raw(), &mut message) };
    }

    /// Complete the adapter's one outstanding transmission.
    pub fn cec_transmit_done(&self, result: TransmitResult) {
        // SAFETY: Successful registration retains the helper data until managed
        // connector teardown. The CEC core serializes adapter transmissions.
        unsafe {
            bindings::drm_connector_hdmi_cec_transmit_done(
                self.as_raw(),
                result.status,
                result.arbitration_lost,
                result.nack,
                result.low_drive,
                result.error,
            )
        };
    }

    /// Return the physical address most recently derived from connector EDID.
    pub fn cec_physical_address(&self) -> u16 {
        // SAFETY: Display information belongs to this initialized connector.
        // Volatile access is the Rust-side equivalent of READ_ONCE while EDID
        // probing may update the field under the modeset locks.
        unsafe {
            core::ptr::addr_of!((*self.as_raw()).display_info.source_physical_address)
                .read_volatile()
        }
    }

    /// Publish or invalidate the adapter's EDID-derived physical address.
    pub fn cec_set_physical_address_valid(&self, valid: bool) {
        // SAFETY: The connector owns registered CEC helper callbacks. These
        // native helpers serialize the adapter update with the CEC core.
        unsafe {
            if valid {
                bindings::drm_connector_cec_phys_addr_set(self.as_raw());
            } else {
                bindings::drm_connector_cec_phys_addr_invalidate(self.as_raw());
            }
        }
    }
}

impl<T: DriverCec> UnregisteredConnector<T> {
    /// Register a managed CEC adapter for this connector.
    pub fn register_cec(
        &self,
        name: &CStr,
        available_logical_addresses: u8,
        parent: &device::Device,
    ) -> Result<ARef<Connector<T>>> {
        if available_logical_addresses > bindings::CEC_MAX_LOG_ADDRS as u8 {
            return Err(EINVAL);
        }
        // SAFETY: This unregistered connector and its parent device remain alive
        // through DRM managed teardown. The static callback table casts only to
        // the matching `Connector<T>` allocation.
        to_result(unsafe {
            bindings::drmm_connector_hdmi_cec_register(
                self.as_raw(),
                &Connector::<T>::CEC_OPS,
                name.as_char_ptr(),
                available_logical_addresses,
                parent.as_raw(),
            )
        })?;
        Ok(ARef::from(&self.0))
    }
}

unsafe extern "C" fn cec_init_callback<T: DriverCec>(
    connector: *mut bindings::drm_connector,
) -> i32 {
    // SAFETY: The callback table is installed only on `Connector<T>`.
    T::cec_init(unsafe { Connector::<T>::from_raw(connector) })
        .map_or_else(|error| error.to_errno(), |_| 0)
}

unsafe extern "C" fn cec_uninit_callback<T: DriverCec>(connector: *mut bindings::drm_connector) {
    // SAFETY: The callback table is installed only on `Connector<T>` and managed
    // teardown invokes uninit before releasing the connector allocation.
    T::cec_uninit(unsafe { Connector::<T>::from_raw(connector) });
}

unsafe extern "C" fn cec_enable_callback<T: DriverCec>(
    connector: *mut bindings::drm_connector,
    enable: bool,
) -> i32 {
    // SAFETY: The callback table is installed only on `Connector<T>`.
    T::cec_enable(unsafe { Connector::<T>::from_raw(connector) }, enable)
        .map_or_else(|error| error.to_errno(), |_| 0)
}

unsafe extern "C" fn cec_logical_address_callback<T: DriverCec>(
    connector: *mut bindings::drm_connector,
    address: u8,
) -> i32 {
    // SAFETY: The callback table is installed only on `Connector<T>`.
    T::cec_logical_address(unsafe { Connector::<T>::from_raw(connector) }, address)
        .map_or_else(|error| error.to_errno(), |_| 0)
}

unsafe extern "C" fn cec_transmit_callback<T: DriverCec>(
    connector: *mut bindings::drm_connector,
    attempts: u8,
    signal_free_time: u32,
    message: *mut bindings::cec_msg,
) -> i32 {
    if message.is_null() {
        return EINVAL.to_errno();
    }
    // SAFETY: The CEC core supplies a valid message for this callback.
    let message = match Message::from_native(unsafe { &*message }) {
        Ok(message) => message,
        Err(error) => return error.to_errno(),
    };
    // SAFETY: The callback table is installed only on `Connector<T>`.
    T::cec_transmit(
        unsafe { Connector::<T>::from_raw(connector) },
        attempts,
        signal_free_time,
        message,
    )
    .map_or_else(|error| error.to_errno(), |_| 0)
}
