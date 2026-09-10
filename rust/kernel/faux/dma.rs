// SPDX-License-Identifier: GPL-2.0-only

//! DMA addressing setup before a new virtual device is exposed to its users.

use super::Registration;
use crate::{
    device,
    dma::DmaMask,
    error::to_result,
    prelude::*, //
};

impl Registration {
    /// Create a faux device with initialized streaming and coherent DMA masks.
    ///
    /// Virtual graphics devices use DMA mapping when attaching imported buffers even without
    /// physical scanout hardware. The selected mask is configured before the caller can create
    /// allocations or publish child devices. Unsupported addressing fails construction and
    /// releases the new registration; an existing device's mask is never changed.
    pub fn new_with_dma_mask(
        name: &CStr,
        parent: Option<&device::Device>,
        mask: DmaMask,
    ) -> Result<Self> {
        let registration = Self::new(name, parent)?;
        // SAFETY: The newly registered faux device has no driver callbacks or DMA users.
        // Native setup stores its mask in the device's stable coherent-mask field before any
        // borrower or child device escapes. Failure drops the private registration.
        to_result(unsafe {
            bindings::dma_coerce_mask_and_coherent(
                registration.as_ref().as_ref().as_raw(),
                mask.value(),
            )
        })?;
        Ok(registration)
    }
}

#[cfg(all(CONFIG_KUNIT, CONFIG_HAS_DMA))]
#[kunit_tests(rust_faux_dma)]
mod tests {
    use super::*;

    #[test]
    fn virtual_mask_storage_outlives_constructor_locals() -> Result {
        let registration =
            Registration::new_with_dma_mask(c"rust-faux-dma-mask", None, DmaMask::new::<64>())?;
        let raw = registration.as_ref().as_ref().as_raw();
        // SAFETY: The retained device owns its mask storage. No DMA setup mutation or mapping
        // occurs concurrently with these observations of the constructor's result.
        let (pointer, coherent, streaming, expected) = unsafe {
            (
                (*raw).dma_mask,
                (*raw).coherent_dma_mask,
                *(*raw).dma_mask,
                &raw mut (*raw).coherent_dma_mask,
            )
        };
        assert_eq!(pointer, expected);
        assert_eq!(coherent, u64::MAX);
        assert_eq!(streaming, u64::MAX);
        Ok(())
    }
}
