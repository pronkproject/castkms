// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Preparation-time producer records, independent of the native wait fence.

use super::*;
use crate::{
    dma_fence::Fence,
    dma_resv::Usage,
    drm::gem::BaseObject, //
};

/// The exact records acquired for one framebuffer preparation.
///
/// Reservation snapshots omit already-signaled records and cannot recover historical errors.
/// Retained records describe completion, not immutable contents or source-read authority.
pub struct Dependencies {
    records: KVec<ARef<Fence>>,
}

impl Dependencies {
    /// Acquire dependencies using the native GEM explicit-versus-implicit policy.
    ///
    /// An explicit producer fence selects only mandatory KERNEL reservation dependencies;
    /// without one, WRITE dependencies are included as well. Every format plane participates.
    /// Each reservation is sampled once; the collection is not a global atomic snapshot and
    /// does not close admission of later submissions.
    pub fn acquire<T: KmsDriver>(
        framebuffer: &Framebuffer<T>,
        explicit: Option<ARef<Fence>>,
    ) -> Result<Self>
    where
        T::Object: BaseObject,
    {
        let usage = if explicit.is_some() {
            Usage::Kernel
        } else {
            Usage::Write
        };
        let mut records = KVec::new();
        if let Some(explicit) = explicit {
            records.push(explicit, GFP_KERNEL)?;
        }
        for plane in 0..framebuffer.plane_count() {
            let reservation = framebuffer.object_at(plane)?.reservation();
            let mut sampled = false;
            for previous in 0..plane {
                if core::ptr::eq(reservation, framebuffer.object_at(previous)?.reservation()) {
                    sampled = true;
                    break;
                }
            }
            if sampled {
                continue;
            }
            let snapshot = reservation.snapshot(usage)?;
            for fence in snapshot.iter() {
                records.push(fence.to_owned_ref(), GFP_KERNEL)?;
            }
        }
        Ok(Self { records })
    }

    /// Retain all acquired records, including errors discarded by native wait merging.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Fence> {
        self.records.iter().map(|fence| &**fence)
    }

    /// Build a native wait from these records without taking another reservation snapshot.
    ///
    /// Its status is not a substitute for inspecting the original producer records.
    pub fn completion(&self) -> Result<Option<ARef<Fence>>> {
        Fence::merge_completion(&self.records)
    }
}
