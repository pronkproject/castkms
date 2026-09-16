// SPDX-License-Identifier: GPL-2.0-only

//! CRTC-driven availability of a CastKMS audio link.

use kernel::{
    prelude::*,
    sync::{Arc, SpinLockIrq},
};

pub(super) struct GateState {
    pub(super) enabled: bool,
    pub(super) generation: u64,
}

/// CRTC-driven playback availability, independent of card and file lifetime.
///
/// Any CRTC availability transition interrupts a prepared playback epoch,
/// including disable/enable pairs occurring between timer callbacks.
#[pin_data]
pub(crate) struct Gate {
    #[pin]
    pub(super) state: SpinLockIrq<GateState>,
}

impl Gate {
    /// Create a gate with driver-selected initial availability.
    pub(crate) fn new(enabled: bool) -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                state <- kernel::new_spinlock_irq!(GateState { enabled, generation: 0 }),
            }),
            GFP_KERNEL,
        )
    }

    /// Publish an availability transition without sleeping or invoking ALSA callbacks.
    pub(crate) fn set_enabled(&self, enabled: bool) {
        let mut state = self.state.lock();
        if state.enabled != enabled {
            match state.generation.checked_add(1) {
                Some(next) => {
                    state.generation = next;
                    state.enabled = enabled;
                }
                None => state.enabled = false,
            }
        }
    }
}
