// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Exporter failures and exact begin/end/unmap ordering for CPU writes.

use super::*;
use crate::{
    error::from_err_ptr,
    sync::{
        aref::ARef,
        Arc, //
    },
    types::Opaque, //
};
use core::{
    ptr::NonNull,
    sync::atomic::{
        AtomicU32,
        Ordering, //
    }, //
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    NoMapping,
    BeginError,
    EndError,
    IoMemory,
}

struct Export {
    bytes: Opaque<[u8; 64]>,
    events: Arc<AtomicU32>,
    mode: Mode,
    direction: bindings::dma_data_direction,
}

impl Export {
    fn record(&self, event: u32) {
        let _ = self
            .events
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                old.checked_mul(10)?.checked_add(event)
            });
    }
}

/// Borrow the private payload during a native exporter callback.
///
/// # Safety
///
/// `buffer` must belong to this test exporter and retain its payload for `'a`.
unsafe fn export<'a>(buffer: *mut bindings::dma_buf) -> &'a Export {
    // SAFETY: Every caller is a callback on the private exporter below. Native ownership
    // retains the exact Export allocation until release; callers do not retain this borrow.
    unsafe { &*(*buffer).priv_.cast::<Export>() }
}

unsafe extern "C" fn map_dma(
    _: *mut bindings::dma_buf_attachment,
    _: bindings::dma_data_direction,
) -> *mut bindings::sg_table {
    EOPNOTSUPP.to_ptr()
}

unsafe extern "C" fn unmap_dma(
    _: *mut bindings::dma_buf_attachment,
    _: *mut bindings::sg_table,
    _: bindings::dma_data_direction,
) {
}

unsafe extern "C" fn release(buffer: *mut bindings::dma_buf) {
    // SAFETY: Native final release transfers the export's sole allocation exactly once.
    let data = unsafe { KBox::from_raw((*buffer).priv_.cast::<Export>()) };
    data.record(5);
}

unsafe extern "C" fn vmap(buffer: *mut bindings::dma_buf, map: *mut bindings::iosys_map) -> i32 {
    // SAFETY: Native export ownership and the callback's exclusive output map remain live.
    let data = unsafe { export(buffer) };
    data.record(1);
    // SAFETY: The native caller supplies an exclusive map. Test payload has stable storage
    // and no Rust references to its bytes. The I/O-memory variant is never dereferenced.
    unsafe {
        (*map).__bindgen_anon_1.vaddr = data.bytes.get().cast();
        (*map).is_iomem = data.mode == Mode::IoMemory;
    }
    0
}

unsafe extern "C" fn vunmap(buffer: *mut bindings::dma_buf, _: *mut bindings::iosys_map) {
    // SAFETY: Called only on the live private exporter, after its final native mapping ends.
    unsafe { export(buffer) }.record(4);
}

unsafe extern "C" fn begin(
    buffer: *mut bindings::dma_buf,
    direction: bindings::dma_data_direction,
) -> i32 {
    // SAFETY: Native ownership retains the private exporter during its callback.
    let data = unsafe { export(buffer) };
    data.record(2);
    if direction != data.direction {
        return EINVAL.to_errno();
    }
    if data.mode == Mode::BeginError {
        EINTR.to_errno()
    } else {
        0
    }
}

unsafe extern "C" fn end(
    buffer: *mut bindings::dma_buf,
    direction: bindings::dma_data_direction,
) -> i32 {
    // SAFETY: Native ownership retains the private exporter during its callback.
    let data = unsafe { export(buffer) };
    data.record(3);
    if direction != data.direction {
        return EINVAL.to_errno();
    }
    if data.mode == Mode::EndError {
        EIO.to_errno()
    } else {
        0
    }
}

static OPS: bindings::dma_buf_ops = bindings::dma_buf_ops {
    attach: None,
    detach: None,
    pin: None,
    unpin: None,
    map_dma_buf: Some(map_dma),
    unmap_dma_buf: Some(unmap_dma),
    release: Some(release),
    begin_cpu_access: Some(begin),
    end_cpu_access: Some(end),
    mmap: None,
    vmap: Some(vmap),
    vunmap: Some(vunmap),
};

static NO_MAPPING_OPS: bindings::dma_buf_ops = bindings::dma_buf_ops {
    vmap: None,
    vunmap: None,
    ..OPS
};

fn create(mode: Mode, flags: u32, events: Arc<AtomicU32>) -> Result<ARef<DmaBuf>> {
    let data = KBox::into_raw(KBox::new(
        Export {
            bytes: Opaque::new([0; 64]),
            mode,
            events,
            direction: if flags == bindings::O_RDONLY {
                bindings::dma_data_direction_DMA_FROM_DEVICE
            } else {
                bindings::dma_data_direction_DMA_TO_DEVICE
            },
        },
        GFP_KERNEL,
    )?);
    let info = bindings::dma_buf_export_info {
        exp_name: c"rust-cpu-write-test".as_char_ptr(),
        ops: if mode == Mode::NoMapping {
            &NO_MAPPING_OPS
        } else {
            &OPS
        },
        size: 64,
        flags: flags as _,
        priv_: data.cast(),
        ..Default::default()
    };
    // SAFETY: Stable private payload, permanent built-in callbacks, and an independently
    // allocated native reservation. Success consumes payload ownership; failure does not.
    match from_err_ptr(unsafe { bindings::dma_buf_export(&info) }) {
        Ok(raw) => {
            // SAFETY: Successful native export transfers one non-null DMA-BUF reference.
            Ok(unsafe { DmaBuf::from_owned_raw(NonNull::new_unchecked(raw)) })
        }
        Err(error) => {
            // SAFETY: Failed export did not call release or consume the private payload.
            drop(unsafe { KBox::from_raw(data) });
            Err(error)
        }
    }
}

fn run(mode: Mode, expected: u32, test: impl FnOnce(&DmaBuf) -> Result) -> Result {
    run_with_flags(mode, bindings::O_RDWR, expected, test)
}

fn run_with_flags(
    mode: Mode,
    flags: u32,
    expected: u32,
    test: impl FnOnce(&DmaBuf) -> Result,
) -> Result {
    let events = Arc::new(AtomicU32::new(0), GFP_KERNEL)?;
    let result = {
        let buffer = create(mode, flags, events.clone())?;
        test(&buffer)
    };
    // SAFETY: The callback returned and released all mapping borrows. Drain native file
    // release in the KUnit kernel thread, with no locks held.
    unsafe { bindings::flush_delayed_fput() };
    if events.load(Ordering::Relaxed) != expected {
        pr_err!(
            "CPU write events: {} expected {}\n",
            events.load(Ordering::Relaxed),
            expected
        );
        return Err(EINVAL);
    }
    result
}

#[kunit_tests(rust_dma_buf_cpu_read_lifetime)]
mod reads {
    use super::*;

    #[test]
    fn write_only_export_is_rejected_before_mapping() -> Result {
        run_with_flags(Mode::Normal, bindings::O_WRONLY, 5, |buffer| {
            assert!(matches!(Read::new(buffer), Err(EACCES)));
            Ok(())
        })
    }

    #[test]
    fn bounds_and_finished_intervals_reject_reads() -> Result {
        run_with_flags(Mode::Normal, bindings::O_RDONLY, 12345, |buffer| {
            let read = Read::new(buffer)?;
            let mut bytes = [0xff; 64];
            read.copy_to_slice(0, &mut bytes)?;
            assert_eq!(bytes, [0; 64]);
            assert_eq!(read.copy_to_slice(1, &mut bytes), Err(EINVAL));
            assert_eq!(read.copy_to_slice(usize::MAX, &mut bytes), Err(EOVERFLOW));
            read.finish()?;
            assert_eq!(read.copy_to_slice(0, &mut bytes), Err(EINVAL));
            read.finish()
        })
    }

    #[test]
    fn failed_begin_unmaps_without_ending() -> Result {
        run_with_flags(Mode::BeginError, bindings::O_RDONLY, 1245, |buffer| {
            assert!(matches!(Read::new(buffer), Err(EINTR)));
            Ok(())
        })
    }

    #[test]
    fn failed_end_is_reported_once() -> Result {
        run_with_flags(Mode::EndError, bindings::O_RDONLY, 12345, |buffer| {
            let read = Read::new(buffer)?;
            assert_eq!(read.finish(), Err(EIO));
            assert_eq!(read.finish(), Ok(()));
            Ok(())
        })
    }

    #[test]
    fn io_memory_is_never_read() -> Result {
        run_with_flags(Mode::IoMemory, bindings::O_RDONLY, 145, |buffer| {
            assert!(matches!(Read::new(buffer), Err(EOPNOTSUPP)));
            Ok(())
        })
    }

    #[test]
    fn drop_ends_unfinished_reads() -> Result {
        run_with_flags(Mode::Normal, bindings::O_RDONLY, 12345, |buffer| {
            let _read = Read::new(buffer)?;
            Ok(())
        })
    }
}

#[kunit_tests(rust_dma_buf_cpu_write_lifetime)]
mod cases {
    use super::*;

    #[test]
    fn read_only_export_is_rejected_before_mapping() -> Result {
        run_with_flags(Mode::Normal, bindings::O_RDONLY, 5, |buffer| {
            if matches!(Write::new(buffer), Err(EACCES)) {
                Ok(())
            } else {
                Err(EINVAL)
            }
        })
    }

    #[test]
    fn write_only_export_allows_cpu_writes() -> Result {
        run_with_flags(Mode::Normal, bindings::O_WRONLY, 12345, |buffer| {
            let mut write = Write::new(buffer)?;
            write.copy_from_slice(0, &[0x31; 64])?;
            write.finish()
        })
    }

    #[test]
    fn explicit_finish_flushes_before_unmapping() -> Result {
        run(Mode::Normal, 12345, |buffer| {
            let mut write = Write::new(buffer)?;
            write.copy_from_slice(0, &[0x31; 64])?;
            write.finish()
        })
    }

    #[test]
    fn drop_flushes_before_unmapping() -> Result {
        run(Mode::Normal, 12345, |buffer| {
            let _write = Write::new(buffer)?;
            Ok(())
        })
    }

    #[test]
    fn unsupported_mapping_does_not_begin_cpu_access() -> Result {
        run(Mode::NoMapping, 5, |buffer| {
            if matches!(Write::new(buffer), Err(EINVAL)) {
                Ok(())
            } else {
                Err(EINVAL)
            }
        })
    }

    #[test]
    fn failed_begin_releases_only_the_mapping() -> Result {
        run(Mode::BeginError, 1245, |buffer| {
            if matches!(Write::new(buffer), Err(EINTR)) {
                Ok(())
            } else {
                Err(EINVAL)
            }
        })
    }

    #[test]
    fn failed_end_is_reported_without_repeating_cleanup() -> Result {
        run(Mode::EndError, 12345, |buffer| {
            if Write::new(buffer)?.finish() == Err(EIO) {
                Ok(())
            } else {
                Err(EINVAL)
            }
        })
    }

    #[test]
    fn io_memory_is_rejected_before_begin() -> Result {
        run(Mode::IoMemory, 145, |buffer| {
            if matches!(Write::new(buffer), Err(EOPNOTSUPP)) {
                Ok(())
            } else {
                Err(EINVAL)
            }
        })
    }
}
