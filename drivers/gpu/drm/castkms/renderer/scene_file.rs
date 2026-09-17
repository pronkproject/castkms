// SPDX-License-Identifier: GPL-2.0-only

//! Bounded complete-scene encoding and transactional descriptor publication.

use super::{job::Plane, endpoint::Endpoint};
use crate::scene::Kind;
use kernel::{
    drm::{
        fourcc,
        kms::{
            colorop::Operation,
            plane::{ColorEncoding, ColorRange},
        },
    },
    fs::{file::FileDescriptorReservation, File},
    prelude::*,
    sync::aref::ARef,
    transmute::FromBytes,
    uaccess::{UserPtr, UserSlice},
    uapi,
};

const MAX_BYTES: usize = uapi::DRM_CASTKMS_RENDERER_SCENE_MAX_BYTES as usize;

const _: () = {
    assert!(
        56 + super::description::MAX_LAYERS
            * (144 + super::description::MAX_COLOR_OPERATIONS * 104)
            + 2 * (8 + 256 * 8)
            + 104
            <= MAX_BYTES
    );
    assert!(super::description::MAX_LAYERS == uapi::DRM_CASTKMS_RENDERER_SCENE_MAX_LAYERS as usize);
    assert!(
        super::description::MAX_COLOR_OPERATIONS
            == uapi::DRM_CASTKMS_RENDERER_SCENE_MAX_COLOR_OPS as usize
    );
};

#[repr(C)]
struct Request {
    result: u64,
    target_image_id: u64,
    capacity: u32,
    flags: u32,
    reserved: u64,
}

// SAFETY: All fields are integers accepting every bit pattern.
unsafe impl FromBytes for Request {}

struct Encoding {
    bytes: KVec<u8>,
}

impl Encoding {
    fn new() -> Result<Self> {
        Ok(Self {
            bytes: KVec::with_capacity(MAX_BYTES, GFP_KERNEL)?,
        })
    }

    fn append(&mut self, bytes: &[u8]) -> Result {
        if bytes.len() > MAX_BYTES - self.bytes.len() {
            return Err(E2BIG);
        }
        for byte in bytes {
            self.bytes.push(*byte, GFP_KERNEL)?;
        }
        Ok(())
    }

    fn word(&mut self, value: u32) -> Result {
        self.append(&value.to_ne_bytes())
    }
    fn wide(&mut self, value: u64) -> Result {
        self.append(&value.to_ne_bytes())
    }
    fn patch(&mut self, offset: usize, value: usize) -> Result {
        let value = u32::try_from(value).map_err(|_| EOVERFLOW)?;
        let end = offset.checked_add(4).ok_or(EINVAL)?;
        self.bytes
            .get_mut(offset..end)
            .ok_or(EINVAL)?
            .copy_from_slice(&value.to_ne_bytes());
        Ok(())
    }

    fn matrix(&mut self, matrix: &[u64; 12]) -> Result {
        self.word(uapi::DRM_CASTKMS_RENDERER_COLOR_MATRIX)?;
        self.word(96)?;
        for value in matrix {
            self.wide(*value)?;
        }
        Ok(())
    }

    fn operation(&mut self, operation: &Operation) -> Result {
        let kind = match operation {
            Operation::Bypass => uapi::DRM_CASTKMS_RENDERER_COLOR_BYPASS,
            Operation::SrgbEotf => uapi::DRM_CASTKMS_RENDERER_COLOR_SRGB_EOTF,
            Operation::SrgbInverseEotf => uapi::DRM_CASTKMS_RENDERER_COLOR_SRGB_INVERSE_EOTF,
            Operation::Matrix(matrix) => return self.matrix(matrix),
        };
        self.word(kind)?;
        self.word(0)
    }

    fn lut(&mut self, entries: &[[u16; 3]]) -> Result {
        if entries.is_empty() || entries.len() > 256 {
            return Err(E2BIG);
        }
        self.word(uapi::DRM_CASTKMS_RENDERER_COLOR_LUT)?;
        self.word((entries.len() * 8) as u32)?;
        for entry in entries {
            for channel in entry {
                self.append(&channel.to_ne_bytes())?;
            }
            self.append(&0u16.to_ne_bytes())?;
        }
        Ok(())
    }
}

fn reserve(
    outputs: &mut KVec<(FileDescriptorReservation, ARef<File>)>,
    file: ARef<File>,
) -> Result<i32> {
    let reservation =
        FileDescriptorReservation::get_unused_fd_flags(kernel::fs::file::flags::O_CLOEXEC)?;
    let fd = reservation
        .reserved_fd()
        .try_into()
        .map_err(|_| EOVERFLOW)?;
    outputs.push((reservation, file), GFP_KERNEL)?;
    Ok(fd)
}

pub(super) fn acquire(endpoint: &Endpoint, arg: usize) -> Result {
    const {
        assert!(
            core::mem::size_of::<Request>()
                == core::mem::size_of::<uapi::drm_castkms_renderer_acquire_job>()
        );
        assert!(core::mem::size_of::<uapi::drm_castkms_renderer_scene>() == 56);
        assert!(core::mem::size_of::<uapi::drm_castkms_renderer_layer>() == 144);
    }
    let request = UserSlice::new(UserPtr::from_addr(arg), core::mem::size_of::<Request>())
        .reader()
        .read::<Request>()?;
    if request.result == 0
        || request.target_image_id == 0
        || request.flags != 0
        || request.reserved != 0
        || request.capacity as usize > MAX_BYTES
    {
        return Err(EINVAL);
    }
    let address = usize::try_from(request.result).map_err(|_| EOVERFLOW)?;
    let mut pending = endpoint.begin_source(request.target_image_id)?;
    let producer = pending.producer_completion()?;
    let scene = pending.scene_description()?;
    let mut encoded = Encoding::new()?;
    let mut outputs = KVec::with_capacity(scene.layers.len() * 4 + 1, GFP_KERNEL)?;
    let producer = match producer {
        Some(fence) => reserve(&mut outputs, fence.create_sync_file()?)?,
        None => -1,
    };
    encoded.word(uapi::DRM_CASTKMS_RENDERER_SCENE_VERSION)?;
    encoded.word(0)?;
    encoded.wide(pending.id())?;
    encoded.wide(pending.constraints_id())?;
    encoded.wide(scene.content_serial)?;
    for dimension in scene.output {
        encoded.word(dimension)?;
    }
    encoded.word(scene.layers.len() as u32)?;
    encoded.word(producer as u32)?;
    encoded.word(0)?;
    encoded.word(0)?;
    // Retain one export per distinct GEM object, including aliases across layers.
    let mut buffers = KVec::with_capacity(scene.layers.len() * 4, GFP_KERNEL)?;
    for layer in &scene.layers {
        let start = encoded.bytes.len();
        let framebuffer = layer.framebuffer();
        let geometry = layer.geometry();
        let operations = layer
            .color
            .as_ref()
            .map_or(&[][..], |color| color.operations());
        encoded.word(0)?;
        encoded.word(match layer.kind {
            Kind::Primary => 0,
            Kind::Overlay => 1,
            Kind::Cursor => 2,
        })?;
        encoded.word(layer.zpos)?;
        encoded.word(framebuffer.format())?;
        encoded.wide(framebuffer.modifier().unwrap_or(fourcc::FORMAT_MOD_INVALID))?;
        encoded.word(framebuffer.width())?;
        encoded.word(framebuffer.height())?;
        for value in geometry.source {
            encoded.word(value)?;
        }
        for value in geometry.position {
            encoded.word(value as u32)?;
        }
        for value in geometry.destination {
            encoded.word(value)?;
        }
        encoded.word(match layer.yuv.0 {
            ColorEncoding::Bt601 => uapi::DRM_CASTKMS_YUV_ENCODING_BT601,
            ColorEncoding::Bt709 => uapi::DRM_CASTKMS_YUV_ENCODING_BT709,
            ColorEncoding::Bt2020 => uapi::DRM_CASTKMS_YUV_ENCODING_BT2020,
        })?;
        encoded.word(match layer.yuv.1 {
            ColorRange::Limited => uapi::DRM_CASTKMS_YUV_RANGE_LIMITED,
            ColorRange::Full => uapi::DRM_CASTKMS_YUV_RANGE_FULL,
        })?;
        encoded.word(framebuffer.plane_count() as u32)?;
        encoded.word(operations.len() as u32)?;
        for index in 0..4 {
            let (fd, pitch, offset) = if index < framebuffer.plane_count() {
                let plane = Plane::new(layer, index)?;
                let previous = buffers.iter().find(
                    |(old, _): &&(Plane<'_>, ARef<kernel::dma_buf::DmaBuf>)| {
                        plane.shares_storage_with(old)
                    },
                );
                let buffer = match previous {
                    Some((_, buffer)) => buffer.clone(),
                    None => plane.export()?,
                };
                let fd = reserve(&mut outputs, buffer.to_file())?;
                let metadata = (fd, plane.pitch, plane.offset);
                buffers.push((plane, buffer), GFP_KERNEL)?;
                metadata
            } else {
                (-1, 0, 0)
            };
            encoded.word(fd as u32)?;
            encoded.word(pitch)?;
            encoded.word(offset)?;
            encoded.word(0)?;
        }
        for operation in operations {
            encoded.operation(operation)?;
        }
        encoded.patch(start, encoded.bytes.len() - start)?;
    }
    let mut output_count = 0;
    if let Some(color) = scene.color {
        let (degamma, matrix, gamma) = color.description();
        if let Some(lut) = degamma {
            encoded.lut(lut)?;
            output_count += 1;
        }
        if let Some(matrix) = matrix {
            encoded.matrix(matrix)?;
            output_count += 1;
        }
        if let Some(lut) = gamma {
            encoded.lut(lut)?;
            output_count += 1;
        }
    }
    encoded.patch(48, output_count)?;
    encoded.patch(4, encoded.bytes.len())?;
    if encoded.bytes.len() > request.capacity as usize {
        return Err(ENOSPC);
    }
    UserSlice::new(UserPtr::from_addr(address), encoded.bytes.len())
        .writer()
        .write_slice(&encoded.bytes)?;
    drop(buffers);
    drop(scene);
    pending.publish(move || {
        for (reservation, file) in outputs {
            reservation.fd_install(file);
        }
    })
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_scene_encoding)]
mod tests {
    use super::*;

    #[test]
    fn color_records_preserve_coefficients_and_lut_channels() -> Result {
        let mut encoded = Encoding::new()?;
        let mut matrix = [0; 12];
        matrix[0] = (1 << 63) | (1 << 32);
        matrix[11] = 17;
        encoded.matrix(&matrix)?;
        assert_eq!(encoded.bytes.len(), 104);
        assert_eq!(&encoded.bytes[8..16], &matrix[0].to_ne_bytes());
        assert_eq!(&encoded.bytes[96..104], &17u64.to_ne_bytes());
        encoded.lut(&[[1, 2, 65535]])?;
        assert_eq!(encoded.bytes.len(), 120);
        assert_eq!(&encoded.bytes[112..114], &1u16.to_ne_bytes());
        assert_eq!(&encoded.bytes[116..118], &65535u16.to_ne_bytes());
        assert_eq!(&encoded.bytes[118..120], &[0, 0]);
        encoded.operation(&Operation::SrgbInverseEotf)?;
        assert_eq!(encoded.bytes.len(), 128);
        assert_eq!(encoded.lut(&[]), Err(E2BIG));
        Ok(())
    }

    #[test]
    fn metadata_capacity_is_a_hard_bound() -> Result {
        let mut encoded = Encoding::new()?;
        for _ in 0..MAX_BYTES / 8 {
            encoded.wide(0)?;
        }
        assert_eq!(encoded.word(1), Err(E2BIG));
        assert_eq!(encoded.bytes.len(), MAX_BYTES);
        assert_eq!(encoded.patch(MAX_BYTES, 0), Err(EINVAL));
        assert_eq!(encoded.patch(usize::MAX, 0), Err(EINVAL));
        Ok(())
    }
}
