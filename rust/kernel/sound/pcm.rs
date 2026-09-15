// SPDX-License-Identifier: GPL-2.0

//! ALSA playback registration, callbacks and synchronized native buffer access.

mod configuration;
pub use configuration::{Config, Format, Identity};

use crate::{
    device::Device,
    error::to_result,
    prelude::*,
    str::CStr,
    sync::{Arc, ArcBorrow, Mutex, SpinLockIrq},
    ThisModule,
};
use core::ptr;

/// Finalized geometry of one prepared native buffer.
#[derive(Clone, Copy)]
pub struct Parameters {
    /// Buffer capacity in interleaved frames.
    pub buffer_frames: usize,
    /// Notification period in interleaved frames.
    pub period_frames: usize,
}

/// Native playback state transitions, called with ALSA's stream lock held.
pub enum Trigger {
    /// Begin a prepared stream.
    Start,
    /// Resume a suspended stream.
    Resume,
    /// Release a paused stream.
    PauseRelease,
    /// Stop playback.
    Stop,
    /// Suspend playback.
    Suspend,
    /// Pause playback.
    PausePush,
}

/// Driver callbacks for a playback PCM. No timing or interruption policy is imposed.
///
/// `prepare`, `stop` and `disconnect` run in sleepable context, serialized by
/// registration. `trigger` and `position` must not sleep. They run
/// under ALSA's stream lock and must not invoke stream notifications recursively.
pub trait Operations: Send + Sync + Sized {
    /// Initialize a newly prepared buffer. Retained streams are invalidated on retirement.
    fn prepare(self: &Arc<Self>, stream: Stream, parameters: Parameters) -> Result;
    /// Quiesce work before buffer replacement, release, close or another preparation.
    fn stop(&self);
    /// Registration is ending; existing native files are about to be disconnected.
    fn disconnect(&self) {
        self.stop();
    }
    /// Respond to a native stream transition.
    fn trigger(&self, command: Trigger) -> Result;
    /// Absolute playback position, in frames; the adapter handles native ring wraparound.
    fn position(&self) -> u64;
}

struct Buffer {
    raw: *mut bindings::snd_pcm_substream,
    generation: u64,
}

// SAFETY: The pointer is accessed only under its owning IRQ-safe lock. Retirement
// clears both published pointers before ALSA can free or replace native storage.
unsafe impl Send for Buffer {}

#[pin_data]
struct Shared {
    config: Config,
    #[pin]
    buffer: SpinLockIrq<Buffer>,
    // Separate from buffer access: notifications invoke driver callbacks, which
    // may lock driver state also held by a caller copying playback samples.
    #[pin]
    notifications: SpinLockIrq<Buffer>,
}

struct Lifecycle {
    closed: bool,
    generation: u64,
}

#[pin_data]
struct State<T: Operations> {
    driver: Arc<T>,
    shared: Arc<Shared>,
    #[pin]
    lifecycle: Mutex<Lifecycle>,
}

impl<T: Operations> State<T> {
    fn retire(&self) {
        self.shared.notifications.lock().raw = ptr::null_mut();
        self.shared.buffer.lock().raw = ptr::null_mut();
    }
}

/// Access to exactly one preparation of an ALSA buffer.
///
/// Retaining a stream does not retain card registration or driver callbacks.
/// Stale operations fail after reconfiguration, close or disconnect.
#[derive(Clone)]
pub struct Stream {
    shared: Arc<Shared>,
    generation: u64,
}

impl Stream {
    /// Copy whole frames from a wrapping native buffer into caller-owned storage.
    ///
    /// The caller chooses the amount of work. Copies hold an IRQ-safe lock and
    /// must therefore be bounded by the driver's latency requirements.
    pub fn copy_frames(&self, start: u64, output: &mut [u8]) -> Result {
        let buffer = self.shared.buffer.lock();
        if buffer.raw.is_null() || buffer.generation != self.generation {
            return Err(ENODEV);
        }
        let frame_bytes = self.shared.config.frame_bytes();
        // SAFETY: Buffer retirement is excluded by the guard and publication occurs
        // only after ALSA allocates and finalizes the runtime buffer.
        let runtime = unsafe { (*buffer.raw).runtime };
        // SAFETY: Finalized buffer geometry remains stable under the buffer guard.
        let size = unsafe { (*runtime).buffer_size as usize };
        if size == 0 || output.len() % frame_bytes != 0 || output.len() / frame_bytes > size {
            return Err(EINVAL);
        }
        let start = (start % size as u64) as usize * frame_bytes;
        for (index, byte) in output.iter_mut().enumerate() {
            // SAFETY: The checked frame range wraps within allocated DMA storage.
            // Volatile loads create no references to memory writable through mmap.
            *byte = unsafe {
                ptr::read_volatile((*runtime).dma_area.add(ring_offset(
                    start,
                    index,
                    size * frame_bytes,
                )))
            };
        }
        Ok(())
    }

    /// Notify ALSA that at least one playback period has elapsed.
    pub fn period_elapsed(&self) -> Result {
        self.notify(false)
    }

    /// Interrupt native playback with an underrun indication.
    pub fn xrun(&self) -> Result {
        self.notify(true)
    }

    fn notify(&self, xrun: bool) -> Result {
        let buffer = self.shared.notifications.lock();
        if buffer.raw.is_null() || buffer.generation != self.generation {
            return Err(ENODEV);
        }
        // SAFETY: The notification guard excludes native runtime retirement, including
        // completion of any driver callbacks invoked by ALSA under its stream lock.
        unsafe {
            if xrun {
                bindings::snd_pcm_stop_xrun(buffer.raw);
            } else {
                bindings::snd_pcm_period_elapsed(buffer.raw);
            }
        }
        Ok(())
    }
}

/// Unique registration of a playback PCM with driver callbacks.
pub struct Registration<T: Operations> {
    raw: *mut bindings::snd_card,
    state: Arc<State<T>>,
}

// SAFETY: Registration is unique, native callbacks retain State, and operations
// on the shared registration serialize with native lifetime transitions.
unsafe impl<T: Operations> Send for Registration<T> {}
// SAFETY: Shared references cannot mutate the native card registration.
unsafe impl<T: Operations> Sync for Registration<T> {}

impl<T: Operations> Registration<T> {
    /// Register one playback PCM. ALSA files retain `module` until their callbacks end.
    pub fn new(
        parent: &Device,
        module: &'static ThisModule,
        identity: Identity<'_>,
        config: Config,
        driver: Arc<T>,
    ) -> Result<Self> {
        config.validate()?;
        let shared = Arc::pin_init(
            pin_init!(Shared {
                config,
                buffer <- crate::new_spinlock_irq!(Buffer { raw: ptr::null_mut(), generation: 0 }),
                notifications <- crate::new_spinlock_irq!(Buffer { raw: ptr::null_mut(), generation: 0 }),
            }),
            GFP_KERNEL,
        )?;
        let state = Arc::pin_init(
            pin_init!(State {
                driver, shared,
                lifecycle <- crate::new_mutex!(Lifecycle { closed: false, generation: 0 }),
            }),
            GFP_KERNEL,
        )?;
        let mut raw = ptr::null_mut();
        // SAFETY: ALSA copies the ID and retains the parent; the output is writable.
        to_result(unsafe {
            bindings::snd_card_new(
                parent.as_raw(),
                -1,
                identity.id.as_char_ptr(),
                module.as_ptr(),
                0,
                &mut raw,
            )
        })?;
        let card = Self {
            raw,
            state,
        };
        // SAFETY: The unregistered card is uniquely owned. private_free consumes the
        // transferred state reference on destruction, including construction failure.
        unsafe {
            (*raw).private_data = Arc::into_raw(card.state.clone()).cast_mut().cast();
            (*raw).private_free = Some(free::<T>);
            copy_name(&mut (*raw).driver, identity.driver);
            copy_name(&mut (*raw).shortname, identity.name);
            copy_name(&mut (*raw).longname, identity.name);
            let mut pcm = ptr::null_mut();
            to_result(bindings::snd_pcm_new(
                raw,
                identity.name.as_char_ptr(),
                0,
                1,
                0,
                &mut pcm,
            ))?;
            (*pcm).private_data = (*raw).private_data;
            copy_name(&mut (*pcm).name, identity.name);
            bindings::snd_pcm_set_ops(pcm, bindings::SNDRV_PCM_STREAM_PLAYBACK as _, &Self::OPS);
            bindings::snd_pcm_lib_preallocate_pages_for_all(
                pcm,
                bindings::SNDRV_DMA_TYPE_VMALLOC as _,
                ptr::null_mut(),
                0,
                0,
            );
            to_result(bindings::snd_card_register(raw))?;
        }
        Ok(card)
    }

    const OPS: bindings::snd_pcm_ops = bindings::snd_pcm_ops {
        open: Some(open::<T>),
        close: Some(close::<T>),
        hw_params: Some(hw_params::<T>),
        hw_free: Some(hw_free::<T>),
        sync_stop: Some(sync_stop::<T>),
        prepare: Some(prepare::<T>),
        trigger: Some(trigger::<T>),
        pointer: Some(pointer::<T>),
        ..pin_init::zeroed()
    };
}

impl<T: Operations> Drop for Registration<T> {
    fn drop(&mut self) {
        let mut lifecycle = self.state.lifecycle.lock();
        lifecycle.closed = true;
        self.state.driver.disconnect();
        self.state.retire();
        drop(lifecycle);
        // SAFETY: Registration is unique; ALSA defers destruction until native files
        // close. Published stream handles have already lost access to native pointers.
        unsafe {
            bindings::snd_card_free_when_closed(self.raw);
        }
    }
}

// SAFETY contract: all callback entry points receive a live substream created
// by Registration<T>, whose PCM private pointer retains State<T>.
unsafe fn state<'a, T: Operations>(
    substream: *mut bindings::snd_pcm_substream,
) -> ArcBorrow<'a, State<T>> {
    // SAFETY: Caller supplies that live substream for the returned borrow's lifetime.
    unsafe { ArcBorrow::from_raw((*(*substream).pcm).private_data.cast()) }
}

unsafe extern "C" fn free<T: Operations>(card: *mut bindings::snd_card) {
    // SAFETY: ALSA transfers the single retained reference at final card destruction.
    drop(unsafe { Arc::<State<T>>::from_raw((*card).private_data.cast()) });
}

unsafe extern "C" fn open<T: Operations>(substream: *mut bindings::snd_pcm_substream) -> c_int {
    // SAFETY: ALSA supplies our live substream and exclusive runtime initialization.
    let state = unsafe { state::<T>(substream) };
    let lifecycle = state.lifecycle.lock();
    if lifecycle.closed {
        return ENODEV.to_errno();
    }
    // SAFETY: The runtime is exclusively initialized for the live native file.
    unsafe {
        (*(*substream).runtime).hw = state.shared.config.hardware();
        bindings::snd_pcm_hw_constraint_integer(
            (*substream).runtime,
            bindings::SNDRV_PCM_HW_PARAM_PERIODS as _,
        )
    }
}

unsafe extern "C" fn hw_params<T: Operations>(
    substream: *mut bindings::snd_pcm_substream,
    params: *mut bindings::snd_pcm_hw_params,
) -> c_int {
    // SAFETY: ALSA supplies a live substream and finalized parameters.
    let state = unsafe { state::<T>(substream) };
    let lifecycle = state.lifecycle.lock();
    if lifecycle.closed {
        return ENODEV.to_errno();
    }
    state.driver.stop();
    state.retire();
    // SAFETY: Private buffer access and notifications are unpublished before
    // allocation, including replacement of a PREPARED buffer without hw_free.
    unsafe {
        let index = (bindings::SNDRV_PCM_HW_PARAM_BUFFER_BYTES
            - bindings::SNDRV_PCM_HW_PARAM_FIRST_INTERVAL) as usize;
        bindings::snd_pcm_lib_malloc_pages(substream, (*params).intervals[index].min as usize)
            .min(0)
    }
}

unsafe extern "C" fn sync_stop<T: Operations>(
    substream: *mut bindings::snd_pcm_substream,
) -> c_int {
    // SAFETY: ALSA supplies our live substream in sleepable context.
    let state = unsafe { state::<T>(substream) };
    let _lifecycle = state.lifecycle.lock();
    state.driver.stop();
    state.retire();
    0
}

unsafe extern "C" fn close<T: Operations>(substream: *mut bindings::snd_pcm_substream) -> c_int {
    // SAFETY: Final close has the same live, sleepable callback lifetime.
    unsafe { sync_stop::<T>(substream) }
}

unsafe extern "C" fn hw_free<T: Operations>(substream: *mut bindings::snd_pcm_substream) -> c_int {
    // SAFETY: ALSA supplies our live substream in sleepable context.
    let state = unsafe { state::<T>(substream) };
    let _lifecycle = state.lifecycle.lock();
    state.driver.stop();
    state.retire();
    // SAFETY: Both kinds of published native access have completed before freeing.
    unsafe { bindings::snd_pcm_lib_free_pages(substream) }
}

unsafe extern "C" fn prepare<T: Operations>(substream: *mut bindings::snd_pcm_substream) -> c_int {
    // SAFETY: ALSA supplies a live, allocated and finalized runtime.
    let state = unsafe { state::<T>(substream) };
    let mut lifecycle = state.lifecycle.lock();
    state.driver.stop();
    state.retire();
    if lifecycle.closed {
        return ENODEV.to_errno();
    }
    let Some(generation) = lifecycle.generation.checked_add(1) else {
        return EOVERFLOW.to_errno();
    };
    lifecycle.generation = generation;
    // SAFETY: ALSA finalized these values before invoking prepare.
    let parameters = unsafe {
        Parameters {
            buffer_frames: (*(*substream).runtime).buffer_size as usize,
            period_frames: (*(*substream).runtime).period_size as usize,
        }
    };
    *state.shared.buffer.lock() = Buffer {
        raw: substream,
        generation,
    };
    *state.shared.notifications.lock() = Buffer {
        raw: substream,
        generation,
    };
    let result = state.driver.prepare(
        Stream {
            shared: state.shared.clone(),
            generation,
        },
        parameters,
    );
    if result.is_err() {
        state.driver.stop();
        state.retire();
    }
    result.map_or_else(|error| error.to_errno(), |()| 0)
}

unsafe extern "C" fn trigger<T: Operations>(
    substream: *mut bindings::snd_pcm_substream,
    command: c_int,
) -> c_int {
    let command = match command as u32 {
        bindings::SNDRV_PCM_TRIGGER_START => Trigger::Start,
        bindings::SNDRV_PCM_TRIGGER_RESUME => Trigger::Resume,
        bindings::SNDRV_PCM_TRIGGER_PAUSE_RELEASE => Trigger::PauseRelease,
        bindings::SNDRV_PCM_TRIGGER_STOP => Trigger::Stop,
        bindings::SNDRV_PCM_TRIGGER_SUSPEND => Trigger::Suspend,
        bindings::SNDRV_PCM_TRIGGER_PAUSE_PUSH => Trigger::PausePush,
        _ => return EINVAL.to_errno(),
    };
    // SAFETY: ALSA retains the substream and holds its native stream lock.
    unsafe { state::<T>(substream) }
        .driver
        .trigger(command)
        .map_or_else(|error| error.to_errno(), |()| 0)
}

unsafe extern "C" fn pointer<T: Operations>(
    substream: *mut bindings::snd_pcm_substream,
) -> bindings::snd_pcm_uframes_t {
    // SAFETY: ALSA retains runtime throughout this stream-locked callback.
    let size = unsafe { (*(*substream).runtime).buffer_size as u64 };
    // SAFETY: The driver state has the same callback lifetime.
    if size == 0 {
        0
    } else {
        (unsafe { state::<T>(substream) }.driver.position() % size) as _
    }
}

fn copy_name(output: &mut [c_char], name: &CStr) {
    output.fill(0);
    let len = name.to_bytes().len().min(output.len().saturating_sub(1));
    for (target, source) in output[..len].iter_mut().zip(name.to_bytes()) {
        *target = *source as c_char;
    }
}

fn ring_offset(start: usize, index: usize, size: usize) -> usize {
    let tail = size - start;
    if index < tail {
        start + index
    } else {
        index - tail
    }
}

#[cfg(CONFIG_KUNIT)]
#[crate::prelude::kunit_tests(rust_snd_pcm_buffer)]
mod tests {
    use super::*;

    fn shared() -> Result<Arc<Shared>> {
        Arc::pin_init(
            pin_init!(Shared {
                config: Config { format: Format::S16Le, rate: 48_000, channels: 2,
                    buffer_bytes_max: 4096, period_bytes_min: 256, period_bytes_max: 2048,
                    periods_min: 2, periods_max: 16, pause: false },
                buffer <- crate::new_spinlock_irq!(Buffer { raw: ptr::null_mut(), generation: 0 }),
                notifications <- crate::new_spinlock_irq!(Buffer { raw: ptr::null_mut(), generation: 0 }),
            }),
            GFP_KERNEL,
        )
    }

    #[test]
    fn retired_stream_rejects_storage_and_notifications() -> Result {
        let stream = Stream {
            shared: shared()?,
            generation: 0,
        };
        assert_eq!(stream.copy_frames(0, &mut [0; 4]), Err(ENODEV));
        assert_eq!(stream.period_elapsed(), Err(ENODEV));
        assert_eq!(stream.xrun(), Err(ENODEV));
        Ok(())
    }

    #[test]
    fn retained_stream_cannot_follow_a_new_preparation() -> Result {
        let shared = shared()?;
        // An inaccessible sentinel demonstrates rejection before native access.
        let raw = ptr::dangling_mut();
        *shared.buffer.lock() = Buffer { raw, generation: 2 };
        *shared.notifications.lock() = Buffer { raw, generation: 2 };
        let stream = Stream {
            shared,
            generation: 1,
        };
        assert_eq!(stream.copy_frames(0, &mut [0; 4]), Err(ENODEV));
        assert_eq!(stream.period_elapsed(), Err(ENODEV));
        assert_eq!(stream.xrun(), Err(ENODEV));
        Ok(())
    }

    #[test]
    fn ring_offsets_do_not_overflow_for_large_buffers() {
        assert_eq!(ring_offset(90, 9, 100), 99);
        assert_eq!(ring_offset(90, 10, 100), 0);
        assert_eq!(ring_offset(usize::MAX - 8, 16, usize::MAX), 8);
    }
}
