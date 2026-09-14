// SPDX-License-Identifier: GPL-2.0 OR MIT

//! CPU write intervals over actual private shmem exports, without descriptor transport.

use super::*;
use crate::{
    dma_buf::{
        cpu_access::Write,
        DmaBuf, //
    },
    drm::kms::testing::TestDevice,
    io::{
        Io,
        SysMem, //
    }, //
};

#[track_caller]
fn check(condition: bool) -> Result {
    if condition {
        Ok(())
    } else {
        let location = core::panic::Location::caller();
        pr_err!(
            "CPU write check failed at {}:{}\n",
            location.file(),
            location.line()
        );
        Err(EINVAL)
    }
}

fn with_buffer(test: impl FnOnce(&DmaBuf) -> Result) -> Result {
    let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
    let parent = faux::Registration::new(c"rust-dma-buf-write", None)?;
    let result = (|| {
        let fixture = TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let buffer = fixture.export_dumb(64, 64, 32)?;
        drop(fixture);
        test(&buffer)
    })();
    // SAFETY: All test mappings and exports are dropped before draining file cleanup,
    // with no locks held and the faux parent still bound.
    unsafe { bindings::flush_delayed_fput() };
    check(counts.gem_objects.load(Ordering::Relaxed) == 0)?;
    check(counts.objects.load(Ordering::Relaxed) == 0)?;
    result
}

/// Inspect private test storage with no external devices or concurrent writers.
fn inspect(buffer: &DmaBuf, pixels: &mut [u8]) -> Result {
    if pixels.len() != buffer.size() {
        return Err(EINVAL);
    }
    let mut map = bindings::iosys_map::default();
    // SAFETY: The live, privately exported test shmem supports system-memory mappings.
    crate::error::to_result(unsafe { bindings::dma_buf_vmap_unlocked(buffer.as_raw(), &mut map) })?;
    let result = if map.is_iomem {
        Err(EINVAL)
    } else {
        // SAFETY: This test alone owns the ordinary shmem allocation. Every writer has
        // ended, and the output length equals the mapping's full allocation length.
        let memory = unsafe {
            SysMem::new(core::ptr::slice_from_raw_parts_mut(
                map.__bindgen_anon_1.vaddr.cast::<u8>(),
                pixels.len(),
            ))
        };
        memory.copy_to_slice(pixels);
        Ok(())
    };
    // SAFETY: Release the one mapping acquired above, including the rejected-map path.
    unsafe { bindings::dma_buf_vunmap_unlocked(buffer.as_raw(), &mut map) };
    result
}

#[kunit_tests(rust_drm_dma_buf_cpu_writes)]
mod cases {
    use super::*;

    #[test]
    fn bounded_writes_preserve_surrounding_bytes() -> Result {
        with_buffer(|buffer| {
            let mut pixels = KVVec::new();
            pixels.resize(buffer.size(), 0x91, GFP_KERNEL)?;
            let mut write = Write::new(buffer)?;
            write.copy_from_slice(0, &pixels)?;
            write.copy_from_slice(17, &[1, 2, 3, 4])?;
            write.copy_from_slice(buffer.size() - 1, &[0x42])?;
            write.finish()?;
            inspect(buffer, &mut pixels)?;
            check(pixels[..17].iter().all(|byte| *byte == 0x91))?;
            check(pixels[17..21] == [1, 2, 3, 4])?;
            check(
                pixels[21..pixels.len() - 1]
                    .iter()
                    .all(|byte| *byte == 0x91),
            )?;
            check(pixels[pixels.len() - 1] == 0x42)
        })
    }

    #[test]
    fn invalid_ranges_do_not_modify_the_mapping() -> Result {
        with_buffer(|buffer| {
            let mut pixels = KVVec::new();
            pixels.resize(buffer.size(), 0x2e, GFP_KERNEL)?;
            let mut write = Write::new(buffer)?;
            write.copy_from_slice(0, &pixels)?;
            check(write.copy_from_slice(buffer.size(), &[0xff]) == Err(EINVAL))?;
            check(write.copy_from_slice(usize::MAX, &[0xff]) == Err(EOVERFLOW))?;
            check(write.copy_from_slice(buffer.size() + 1, &[]) == Err(EINVAL))?;
            write.copy_from_slice(buffer.size(), &[])?;
            write.finish()?;
            inspect(buffer, &mut pixels)?;
            check(pixels.iter().all(|byte| *byte == 0x2e))
        })
    }

    #[test]
    fn dropping_an_interval_releases_its_mapping() -> Result {
        with_buffer(|buffer| {
            for value in 0..32 {
                let mut write = Write::new(buffer)?;
                write.copy_from_slice(0, &[value])?;
            }
            let mut pixels = KVVec::new();
            pixels.resize(buffer.size(), 0, GFP_KERNEL)?;
            inspect(buffer, &mut pixels)?;
            check(pixels[0] == 31)
        })
    }
}
