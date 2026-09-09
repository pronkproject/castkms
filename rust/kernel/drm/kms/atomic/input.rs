// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Immutable plane inputs, independent of mutable validation results.

use super::*;

/// A plane's input at the first validation entry, not its eventual display state.
///
/// Kernel construction and userspace property processing have the same representation.
/// Neither inclusion nor framebuffer assignment proves requester identity or authority.
pub enum PlaneInput<'a, T: KmsDriver> {
    /// Validation has not started; no input snapshot exists yet.
    Uncaptured,
    /// Validation started without new state for the plane.
    Omitted,
    /// New state for the plane was present before validation.
    Included {
        /// The input framebuffer, retained even if validation replaces it.
        /// This preserves object identity, not pixels or permission to read them.
        framebuffer: Option<&'a Framebuffer<T>>,
        /// The common setter had been called before validation, including same-image or
        /// null assignments. Helpers during kernel request construction also count.
        framebuffer_assigned: bool,
    },
}

impl<T: KmsDriver> AtomicState<T> {
    fn plane_input<P>(&self, plane: &P) -> Result<PlaneInput<'_, T>>
    where
        P: ModesettablePlane + ModeObject<Driver = T>,
    {
        if !core::ptr::eq(self.drm_dev(), plane.drm_dev()) {
            return Err(EINVAL);
        }
        // SAFETY: The typed plane belongs to this transaction's device. Input records live
        // until transaction clear, which cannot overlap the transaction borrow. Their
        // framebuffer references are independent of mutable new-plane state.
        let input =
            unsafe { bindings::drm_atomic_get_plane_input(self.as_raw(), plane.as_raw()).as_ref() };
        Ok(match input {
            None => PlaneInput::Uncaptured,
            Some(input) if !input.included => PlaneInput::Omitted,
            Some(input) => PlaneInput::Included {
                // SAFETY: The frozen record retains a framebuffer on the typed device.
                framebuffer: unsafe { input.fb.as_ref().map(|fb| Framebuffer::from_raw(fb)) },
                framebuffer_assigned: input.fb_assigned,
            },
        })
    }
}

impl<T: KmsDriver> AtomicStateMutator<T> {
    /// Inspect frozen plane input without borrowing its mutable validation state.
    ///
    /// A plane from another device returns `EINVAL`. The snapshot is captured once before
    /// validation and remains unchanged across retries on the same transaction. Clearing
    /// the transaction discards it. A new request must be constructed after that clear.
    pub fn plane_input<P>(&self, plane: &P) -> Result<PlaneInput<'_, T>>
    where
        P: ModesettablePlane + ModeObject<Driver = T>,
    {
        self.state.plane_input(plane)
    }
}

impl<T: KmsDriver> AtomicStateReader<T> {
    /// Inspect the input that preceded validation, within this commit callback's lifetime.
    ///
    /// See [`AtomicStateMutator::plane_input`]. The input does not itself authorize capture.
    pub fn plane_input<P>(&self, plane: &P) -> Result<PlaneInput<'_, T>>
    where
        P: ModesettablePlane + ModeObject<Driver = T>,
    {
        self.0.plane_input(plane)
    }
}
