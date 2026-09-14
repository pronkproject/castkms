// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Retained export access modes, without attachment or CPU access.

use super::*;

unsafe extern "C" fn map(
    _: *mut bindings::dma_buf_attachment,
    _: bindings::dma_data_direction,
) -> *mut bindings::sg_table {
    EOPNOTSUPP.to_ptr()
}

unsafe extern "C" fn unmap(
    _: *mut bindings::dma_buf_attachment,
    _: *mut bindings::sg_table,
    _: bindings::dma_data_direction,
) {
}

unsafe extern "C" fn release(_: *mut bindings::dma_buf) {}

static EXPORT_MARKER: u8 = 0;

static OPS: bindings::dma_buf_ops = bindings::dma_buf_ops {
    attach: None,
    detach: None,
    pin: None,
    unpin: None,
    map_dma_buf: Some(map),
    unmap_dma_buf: Some(unmap),
    release: Some(release),
    begin_cpu_access: None,
    end_cpu_access: None,
    mmap: None,
    vmap: None,
    vunmap: None,
};

fn exported(flags: u32) -> Result<ARef<DmaBuf>> {
    let info = bindings::dma_buf_export_info {
        exp_name: c"rust-export-access-test".as_char_ptr(),
        ops: &OPS,
        size: 4096,
        flags: flags as _,
        priv_: (&raw const EXPORT_MARKER).cast_mut().cast(),
        ..Default::default()
    };
    // SAFETY: Permanent built-in callbacks expose no mapping and never mutate or free
    // the static marker. Native export owns its reservation and file metadata.
    let raw = from_err_ptr(unsafe { bindings::dma_buf_export(&info) })?;
    // SAFETY: Successful export transfers one reference to the initialized buffer.
    Ok(unsafe { DmaBuf::from_owned_raw(NonNull::new(raw).ok_or(EINVAL)?) })
}

fn check_mode(flags: u32, writable: bool) -> Result {
    let buffer = exported(flags)?;
    let retained = buffer.clone();
    drop(buffer);
    let matches = retained.is_writable() == writable;
    drop(retained);
    // SAFETY: All local file owners have been released and no locks are held. Drain
    // delayed final fput in the KUnit kernel thread before completing the case.
    unsafe { bindings::flush_delayed_fput() };
    if matches {
        Ok(())
    } else {
        Err(EINVAL)
    }
}

#[kunit_tests(rust_dma_buf_export_access)]
mod cases {
    use super::*;

    #[test]
    fn read_only_export_has_no_file_write_access() -> Result {
        check_mode(bindings::O_RDONLY, false)
    }

    #[test]
    fn write_only_export_retains_file_write_access() -> Result {
        check_mode(bindings::O_WRONLY, true)
    }

    #[test]
    fn read_write_export_retains_file_write_access() -> Result {
        check_mode(bindings::O_RDWR, true)
    }
}
