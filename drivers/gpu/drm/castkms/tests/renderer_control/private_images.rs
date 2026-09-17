// SPDX-License-Identifier: GPL-2.0-only

//! Shared native-endpoint fixtures.

use super::*;
use crate::execution::capabilities::{ColorLimits, Format, GeometryLimits, Limits, Profile};
use kernel::{dma_buf::DmaBuf, drm::gem::{BaseObject, ExportAccess}, sync::aref::ARef};

pub(crate) fn profile() -> Result<Profile> {
    profile_for(None)
}

pub(crate) fn profile_for(modifier: Option<u64>) -> Result<Profile> {
    let mut formats = KVec::new();
    formats.push(Format {
        fourcc: drm::fourcc::XRGB8888,
        modifier,
        planes: 1,
        native: true,
        imported: true,
        width_alignment: 1,
        height_alignment: 1,
        pitch_alignment: 1,
        offset_alignment: 1,
        max_pitch: u32::MAX,
    }, GFP_KERNEL)?;
    Profile::new(Limits {
        geometry: GeometryLimits {
            min_output: [1; 2], min_source: [1; 2], output: [16384; 2], source: [16384; 2],
            crop: true, fractional: true, position: true, scale: true,
            min_scale: 1 << 12, max_scale: 1 << 20,
        },
        color: ColorLimits {
            operations: 16, srgb: true, plane_matrix: true, output_matrix: true,
            lut_entries: 256, yuv_encodings: [true; 3], yuv_ranges: [true; 2],
        },
        layers: 24,
        roles: [1, 22, 1],
    }, formats)
}

pub(super) fn buffer(device: &Device<Driver>, access: ExportAccess) -> Result<ARef<DmaBuf>> {
    shmem::Object::<gem::Object>::new(
        device, 640 * 480 * 4, Default::default(), Default::default(),
    )?.export_dma_buf(access)
}
