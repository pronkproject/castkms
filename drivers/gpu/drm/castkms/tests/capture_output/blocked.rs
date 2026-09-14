// SPDX-License-Identifier: GPL-2.0-only

//! Insert an implicit dependency inside begin_cpu_access, after both readiness snapshots.

use super::*;
use crate::capture::client::Client;
use core::{
    ptr::NonNull,
    sync::atomic::{
        AtomicBool,
        Ordering, //
    }, //
};
use kernel::{
    bindings,
    drm::capture::{
        ClientOwner,
        ClientStream,
        Description,
        Readiness, //
    },
    error::{
        from_err_ptr,
        to_result, //
    },
    sync::Arc,
    time::{
        delay::fsleep,
        Delta,
        Instant,
        Monotonic, //
    },
    types::ScopeGuard, //
};

struct Gate {
    entered: AtomicBool,
    unmapped: AtomicBool,
    fence: ARef<kernel::dma_fence::Fence>,
}

struct Export {
    backing: ARef<DmaBuf>,
    gate: Arc<Gate>,
}

fn raw(buffer: &DmaBuf) -> *mut bindings::dma_buf {
    core::ptr::from_ref(buffer).cast_mut().cast()
}

// SAFETY: Each caller is an exporter callback whose native buffer retains Export.
unsafe fn export<'a>(buffer: *mut bindings::dma_buf) -> &'a Export {
    // SAFETY: Export construction installs this initialized payload until native release.
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
    // SAFETY: Native final release returns the sole payload allocation once.
    drop(unsafe { KBox::from_raw((*buffer).priv_.cast::<Export>()) });
}

unsafe extern "C" fn vmap(buffer: *mut bindings::dma_buf, map: *mut bindings::iosys_map) -> i32 {
    // SAFETY: The wrapper retains its backing allocation. Both exports share the same
    // reservation, already locked by the native vmap caller. Output map is exclusive.
    unsafe { bindings::dma_buf_vmap(raw(&export(buffer).backing), map) }
}

unsafe extern "C" fn vunmap(buffer: *mut bindings::dma_buf, map: *mut bindings::iosys_map) {
    // SAFETY: Native ownership retains Export through this balanced callback.
    let export = unsafe { export(buffer) };
    // SAFETY: Both exports share the caller-held reservation; this balances the backing map.
    unsafe { bindings::dma_buf_vunmap(raw(&export.backing), map) };
    export.gate.unmapped.store(true, Ordering::Release);
}

unsafe extern "C" fn begin(buffer: *mut bindings::dma_buf, _: bindings::dma_data_direction) -> i32 {
    // SAFETY: Native ownership retains this exporter and its shared reservation.
    let export = unsafe { export(buffer) };
    // SAFETY: Published DMA-BUF reservation is immutable and retained by backing storage.
    let reservation = unsafe { (*buffer).resv };
    // SAFETY: CPU begin callbacks run without the reservation lock. Take one lock only.
    let locked = to_result(unsafe { bindings::dma_resv_lock(reservation, core::ptr::null_mut()) });
    if let Err(error) = locked {
        return error.to_errno();
    }
    let _unlock = ScopeGuard::new(|| {
        // SAFETY: Balance the acquired reservation before returning from the callback.
        unsafe { bindings::dma_resv_unlock(reservation) };
    });
    // SAFETY: Reserve fence capacity while the retained reservation is locked.
    let result = to_result(unsafe { bindings::dma_resv_reserve_fences(reservation, 1) });
    if let Err(error) = result {
        return error.to_errno();
    }
    // SAFETY: Fence has transparent native representation and is retained by Gate. Native
    // insertion takes an independent reference and consumes the reserved capacity.
    unsafe {
        bindings::dma_resv_add_fence(
            reservation,
            core::ptr::from_ref(&*export.gate.fence).cast_mut().cast(),
            bindings::dma_resv_usage_DMA_RESV_USAGE_READ,
        );
    }
    export.gate.entered.store(true, Ordering::Release);
    0
}

static OPS: bindings::dma_buf_ops = bindings::dma_buf_ops {
    map_dma_buf: Some(map_dma),
    unmap_dma_buf: Some(unmap_dma),
    release: Some(release),
    begin_cpu_access: Some(begin),
    vmap: Some(vmap),
    vunmap: Some(vunmap),
    // SAFETY: All omitted callbacks are nullable function pointers or boolean flags.
    ..unsafe { core::mem::zeroed() }
};

fn destination_with_gate(fixture: &Fixture, gate: Arc<Gate>) -> Result<(Image, ARef<DmaBuf>)> {
    let image = destination(fixture, Layout::new(640, 480)?)?;
    let backing: ARef<DmaBuf> = image.buffer().into();
    let data = KBox::into_raw(KBox::new(
        Export {
            backing: backing.clone(),
            gate,
        },
        GFP_KERNEL,
    )?);
    let info = bindings::dma_buf_export_info {
        exp_name: c"castkms-blocked-output-test".as_ptr().cast(),
        owner: <crate::CastKms as kernel::ModuleMetadata>::THIS_MODULE.as_ptr(),
        ops: &OPS,
        size: backing.size(),
        flags: bindings::O_RDWR as i32,
        // SAFETY: The transferred Export retains backing and its reservation.
        resv: unsafe { (*raw(&backing)).resv },
        priv_: data.cast(),
    };
    // SAFETY: The immutable description transfers data only on successful export.
    let buffer = match from_err_ptr(unsafe { bindings::dma_buf_export(&info) }) {
        Ok(buffer) => buffer,
        Err(error) => {
            // SAFETY: Rejected export leaves the original allocation with this caller.
            drop(unsafe { KBox::from_raw(data) });
            return Err(error);
        }
    };
    // SAFETY: Successful export transfers one reference to a transparent initialized DmaBuf.
    let buffer = unsafe { ARef::<DmaBuf>::from_raw(NonNull::new(buffer.cast()).ok_or(EIO)?) };
    Ok((
        Image::new(
            buffer,
            image.layout(),
            fourcc::XRGB8888,
            fourcc::FORMAT_MOD_LINEAR,
            2624,
            128,
        )?,
        backing,
    ))
}

fn wait_for(mut ready: impl FnMut() -> bool) -> Result {
    let start = Instant::<Monotonic>::now();
    while !ready() {
        if start.elapsed() > Delta::from_millis(1000) {
            return Err(ETIMEDOUT);
        }
        fsleep(Delta::from_millis(1));
    }
    Ok(())
}

fn gate(fence: &ManualFence) -> Result<Arc<Gate>> {
    Ok(Arc::new(
        Gate {
            entered: AtomicBool::new(false),
            unmapped: AtomicBool::new(false),
            fence: fence.fence(),
        },
        GFP_KERNEL,
    )?)
}

#[kunit_tests(rust_castkms_capture_blocked_output)]
mod cases {
    use super::*;

    #[test]
    fn revoked_file_cancels_blocked_access_without_acknowledging_reuse() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let creator = fixture.drm.master_file()?;
            let _fb = select(fixture, &creator)?;
            let grantor = grant(fixture, &creator)?;
            let mut fence = ManualFence::new()?;
            let gate = gate(&fence)?;
            let (image, backing) = destination_with_gate(fixture, gate.clone())?;
            super::super::file::with_client(grantor.capture(), |file| {
                let _registered = super::super::file::register(file, 1, &image)?;
                let mut stream = ClientStream::open(file, 1, Description::query(file)?.id(), 1)?;
                let readiness = Readiness::for_client(file)?;
                stream.queue_output(1, 1, None)?;
                wait_for(|| gate.entered.load(Ordering::Acquire))?;
                drop(grantor);
                stream.cancel(1)?;
                check(stream.cancel(1) == Err(EALREADY))?;
                check(stream.dequeue(|_| Ok(())) == Err(EAGAIN))?;
                check(!readiness.has_results())?;
                check(stream.close() == Err(EBUSY))?;
                check(stream.queue_output(2, 1, None) == Err(EKEYREVOKED))?;
                check(!gate.unmapped.load(Ordering::Acquire))?;
                fence.complete(Ok(()))?;
                wait_for(|| readiness.has_results())?;
                stream.dequeue(|completion| {
                    check(completion.use_id() == 1)?;
                    check(matches!(completion.result(), Err(ECANCELED)))
                })?;
                check(gate.unmapped.load(Ordering::Acquire))?;
                stream.close()?;
                check(pixels(&backing)?.iter().all(|byte| *byte == 0x73))
            })
        })
    }

    #[test]
    fn a_late_implicit_fence_does_not_lock_out_sibling_streams() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let mut client = Client::new(grantor.capture())?;
            let offer = client.describe()?.id();
            client.open_stream(1, offer, 1)?;
            client.open_stream(2, offer, 1)?;
            let mut fence = ManualFence::new()?;
            let gate = gate(&fence)?;
            let (image, backing) = destination_with_gate(fixture, gate.clone())?;
            client.register_destination(1, image)?;
            client.register_destination(2, destination(fixture, Layout::new(640, 480)?)?)?;
            client.queue_to(1, 1, 1, None)?;
            wait_for(|| gate.entered.load(Ordering::Acquire))?;
            check(client.stream(1)?.advance()? == 0)?;
            let source: ARef<Source> = fixture
                .drm
                .device()
                .output
                .inspect_accepted(|accepted| accepted.map(|(source, _)| source.into()))
                .ok_or(EINVAL)?;
            {
                let admission = source.hold_admission()?;
                check(admission.prepared()?.is_some())?;
            }
            client.queue_to(2, 1, 2, None)?;
            wait_for(|| client.readiness().is_some_and(|ready| ready.has_results()))?;
            client.dequeue(2, |completion| completion.result().map(|_| ()))?;
            client.cancel(1, 1)?;
            check(client.cancel(1, 1) == Err(EALREADY))?;
            check(client.close_stream(1) == Err(EBUSY))?;
            check(client.queue_to(1, 2, 1, None) == Err(ESHUTDOWN))?;
            client.close_stream(2)?;
            check(!gate.unmapped.load(Ordering::Acquire))?;
            fence.complete(Ok(()))?;
            wait_for(|| client.readiness().is_some_and(|ready| ready.has_results()))?;
            client.dequeue(1, |completion| {
                check(matches!(completion.result(), Err(ECANCELED)))
            })?;
            client.close_stream(1)?;
            check(pixels(&backing)?.iter().all(|byte| *byte == 0x73))
        })
    }

    #[test]
    fn final_file_release_detaches_without_waiting_for_destination_access() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let mut fence = ManualFence::new()?;
            let gate = gate(&fence)?;
            let (image, backing) = destination_with_gate(fixture, gate.clone())?;
            let client = grantor.capture().into_client_file_with(|capture| {
                let mut client = Client::new(capture)?;
                let offer = client.describe()?.id();
                client.open_stream(1, offer, 1)?;
                client.register_destination(1, image)?;
                client.queue_to(1, 1, 1, None)?;
                Ok(client)
            })?;
            wait_for(|| gate.entered.load(Ordering::Acquire))?;
            // SAFETY: The KUnit thread transfers the final file reference with no locks held.
            unsafe { bindings::__fput_sync(ARef::into_raw(client).cast().as_ptr()) };
            check(!gate.unmapped.load(Ordering::Acquire))?;
            fence.complete(Ok(()))?;
            wait_for(|| gate.unmapped.load(Ordering::Acquire))?;
            check(pixels(&backing)?.iter().all(|byte| *byte == 0x73))
        })
    }
}
