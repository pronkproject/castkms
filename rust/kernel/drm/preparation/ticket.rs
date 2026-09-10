// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Cancelable request ownership above source admission accounting.

use super::Source;
use crate::{
    error::{
        from_err_ptr,
        to_result, //
    },
    prelude::*,
    sync::aref::{
        ARef,
        AlwaysRefCounted, //
    },
    types::Opaque, //
};
use core::ptr::NonNull;

/// One output's accepted source generation, borrowed for ticket construction.
///
/// The provider supplies a fresh source for every accepted use, including same-framebuffer
/// updates and blank outputs, and holds display locks while collecting the complete list.
#[derive(Clone, Copy)]
pub struct OutputGeneration<'a> {
    crtc_id: core::num::NonZeroU32,
    source: &'a Source,
}

impl<'a> OutputGeneration<'a> {
    /// Describe an output without granting display authority or pixel access.
    pub fn new(crtc_id: u32, source: &'a Source) -> Result<Self> {
        Ok(Self {
            crtc_id: core::num::NonZeroU32::new(crtc_id).ok_or(EINVAL)?,
            source,
        })
    }
}

/// An observation of submission preparation, independent of native GPU completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TicketStatus {
    /// At least one admitted read claim has not been relinquished.
    Pending,
    /// Claims have been relinquished; another attempt may still own the reservation.
    Ready,
    /// An accepted transaction consumed the ticket and retains retirement ownership.
    Consumed,
    /// The ticket no longer permits acceptance; existing attempts retain their holds.
    Canceled,
    /// Abandoned source access prevents establishing submission closure.
    Failed,
}

/// A shared, explicitly cancelable preparation request for exact output generations.
///
/// Dropping a reference is not cancellation. A transport or authority owner must call
/// [`Self::cancel`] at its specified lifetime boundary. Cancellation cannot release ownership
/// retained by an active attempt or accepted display update. The ticket does not authenticate
/// callers or grant pixel access. Native acceptance compares the complete observed output
/// generations with the captured list. All operations may sleep.
///
/// # Invariants
///
/// Every reference retains an initialized native ticket with a live reference count.
#[repr(transparent)]
pub struct Ticket(Opaque<bindings::drm_prepare_ticket>);

// SAFETY: Native reference counting and cancellation support ownership across tasks.
unsafe impl Send for Ticket {}
// SAFETY: Shared operations serialize cancellation and reservation through the native mutex.
unsafe impl Sync for Ticket {}

// SAFETY: Native get/put retain and release the initialized ticket allocation.
unsafe impl AlwaysRefCounted for Ticket {
    fn inc_ref(&self) {
        // SAFETY: The shared reference retains a live initialized ticket.
        unsafe { bindings::drm_prepare_ticket_get(self.0.get()) };
    }

    unsafe fn dec_ref(ptr: NonNull<Self>) {
        // SAFETY: The caller transfers one reference to the identically represented ticket.
        unsafe { bindings::drm_prepare_ticket_put(ptr.as_ptr().cast()) };
    }
}

impl Ticket {
    pub(super) fn as_raw(&self) -> *mut bindings::drm_prepare_ticket {
        self.0.get()
    }

    /// Capture output generations and hold admission for their sources.
    ///
    /// Read claims may still be pending. Ticket construction does not establish readiness or
    /// wait for completion; reservation checks that condition before creating an attempt.
    /// IDs must be unique and sources must share an admission domain. An empty list matches
    /// only a transaction retiring no outputs; it is not a wildcard. The caller stabilizes
    /// the complete list with its display locks during construction.
    pub fn new(outputs: &[OutputGeneration<'_>]) -> Result<ARef<Self>> {
        let mut entries = [bindings::drm_prepare_output_generation {
            crtc_id: 0,
            source: core::ptr::null_mut(),
        }; bindings::DRM_PREPARE_MAX_OUTPUTS as usize];
        if outputs.len() > entries.len() {
            return Err(E2BIG);
        }
        for (entry, output) in entries.iter_mut().zip(outputs) {
            entry.crtc_id = output.crtc_id.get();
            entry.source = output.source.0.get();
        }
        // SAFETY: Entries borrow initialized sources throughout native construction, which
        // copies the list and takes independent source references and admission holds.
        let ticket = from_err_ptr(unsafe {
            bindings::drm_prepare_ticket_create(entries.as_ptr(), outputs.len() as u32)
        })?;
        // SAFETY: Successful creation transfers a non-null initialized native reference.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(ticket.cast())) })
    }

    /// Prevent new acceptance without canceling already submitted native reads.
    ///
    /// Repeated cancellation is harmless. An accepted guard remains independently owned.
    pub fn cancel(&self) {
        // SAFETY: The shared reference retains the ticket; native cancellation is serialized.
        unsafe { bindings::drm_prepare_ticket_cancel(self.0.get()) };
    }

    /// Observe ticket status without waiting for read claims or native GPU completion.
    ///
    /// The query may sleep while taking internal locks. It neither reserves an attempt nor
    /// validates output generations. Cancellation or consumption can follow the observation;
    /// even [`TicketStatus::Ready`] requires [`Self::reserve`] to recheck availability.
    pub fn status(&self) -> TicketStatus {
        // SAFETY: The shared reference retains the initialized ticket during the query.
        match unsafe { bindings::drm_prepare_ticket_status(self.0.get()) } {
            bindings::drm_prepare_ticket_status_DRM_PREPARE_TICKET_PENDING => TicketStatus::Pending,
            bindings::drm_prepare_ticket_status_DRM_PREPARE_TICKET_READY => TicketStatus::Ready,
            bindings::drm_prepare_ticket_status_DRM_PREPARE_TICKET_CONSUMED => {
                TicketStatus::Consumed
            }
            bindings::drm_prepare_ticket_status_DRM_PREPARE_TICKET_CANCELED => {
                TicketStatus::Canceled
            }
            _ => TicketStatus::Failed,
        }
    }

    /// Wait interruptibly for readiness or cancellation without reserving an attempt.
    ///
    /// Success is an observation, not permission to install an update: [`Self::reserve`] still
    /// checks exclusive ownership and ticket lifetime. Cancellation wakes a pending wait with
    /// ECANCELED; consumed tickets return EALREADY. GPU reads need not have completed. The wait
    /// temporarily retains admission until it returns, including after cancellation. Do not
    /// hold locks needed by claim owners or cancellation while waiting.
    pub fn wait_ready(&self) -> Result {
        // SAFETY: The shared reference retains the ticket and native waiting retains its set.
        to_result(unsafe { bindings::drm_prepare_ticket_wait(self.0.get()) })
    }

    /// Reserve one attempt without consuming the cancelable request.
    ///
    /// Pending claims return EAGAIN, another reservation returns EBUSY, and cancellation returns
    /// ECANCELED. All native completion collection precedes publication. Dropping the returned
    /// owner permits retry if the ticket is still live; it never invents successful installation.
    pub fn reserve(&self) -> Result<Attempt> {
        // SAFETY: The shared ticket remains live; reservation takes its own native reference.
        let attempt = from_err_ptr(unsafe { bindings::drm_prepare_ticket_reserve(self.0.get()) })?;
        Ok(Attempt {
            // SAFETY: Successful reservation transfers one non-null, uniquely owned attempt.
            raw: unsafe { NonNull::new_unchecked(attempt) },
        })
    }
}

/// A unique reservation retaining its ticket and preassembled retirement ownership.
///
/// Moving an attempt does not release admission. Dropping it abandons only that attempt,
/// permitting a still-live ticket to retry. Cancellation may prevent acceptance but cannot
/// release this owner's holds. No public method accepts a display update or exposes raw pointers.
/// Destruction requires a context that may sleep.
///
/// # Invariants
///
/// The pointer owns one initialized native attempt, released exactly once by Drop.
pub struct Attempt {
    raw: NonNull<bindings::drm_prepare_attempt>,
}

// SAFETY: Exclusive ownership may move between tasks; native ticket access uses its own mutex.
unsafe impl Send for Attempt {}

impl Drop for Attempt {
    fn drop(&mut self) {
        // SAFETY: Drop exclusively consumes the native attempt and its retained references.
        unsafe { bindings::drm_prepare_attempt_destroy(self.raw.as_ptr()) };
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
