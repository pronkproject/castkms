// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Native DRM timers for controllers without a physical vblank interrupt.

use super::{
    DriverCrtc,
    VblankImpl,
    VblankOps, //
};
use core::marker::PhantomData;

/// Select the native DRM timer as a controller's vblank source.
///
/// Use this as [`DriverCrtc::VblankImpl`]. The timer follows the programmed display mode and
/// advances the native vblank counter without reading pixels. The driver must enable and disable
/// vblank in its atomic callbacks and arm or send pending events after publishing its update.
///
/// DRM initializes the timer on first use and cancels it during managed vblank cleanup. The
/// native callbacks remain callable only through DRM's callback table, under its locking and
/// initialization protocol; this marker exposes no separate start or cancel operation.
pub struct SoftwareVblank<T>(PhantomData<T>);

impl<T: DriverCrtc<VblankImpl = Self>> VblankImpl for SoftwareVblank<T> {
    type Crtc = T;

    const VBLANK_OPS: VblankOps = VblankOps {
        enable_vblank: Some(bindings::drm_crtc_vblank_helper_enable_vblank_timer),
        disable_vblank: Some(bindings::drm_crtc_vblank_helper_disable_vblank_timer),
        get_vblank_timestamp: Some(
            bindings::drm_crtc_vblank_helper_get_vblank_timestamp_from_timer,
        ),
    };
}
