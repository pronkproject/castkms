// SPDX-License-Identifier: GPL-2.0-only

//! Timer-driven CastKMS playback and display-link interruption policy.

mod clock;
mod link;
pub(crate) use link::Gate;

use super::{FRAME_BYTES, PERIOD_FRAMES, RATE};
use kernel::time::hrtimer::HrTimerExpires;
use kernel::{
    prelude::*,
    sound::pcm::{self, Operations},
    sync::{Arc, ArcBorrow, Mutex, SpinLockIrq},
    time::{
        hrtimer::{
            ArcHrTimerHandle, HrTimer, HrTimerCallback, HrTimerCallbackContext, HrTimerPointer,
            HrTimerRestart, RelativeMode,
        },
        Delta, Instant, Monotonic,
    },
};

const TIMER_INTERVAL_US: i64 = 1_000;
const MAX_READ_FRAMES: usize = 4 * PERIOD_FRAMES;

#[derive(Default)]
pub(super) struct Cursor {
    generation: u64,
    frames: u64,
}

struct Runtime {
    stream: Option<pcm::Stream>,
    parameters: pcm::Parameters,
    clock: clock::Playback,
    closed: bool,
    generation: u64,
    notified: u64,
    link_generation: u64,
}

#[pin_data]
pub(crate) struct Playback {
    link: Arc<Gate>,
    #[pin]
    runtime: SpinLockIrq<Runtime>,
    #[pin]
    timer: HrTimer<Self>,
    #[pin]
    handle: Mutex<Option<ArcHrTimerHandle<Self>>>,
}

kernel::impl_has_hr_timer! {
    impl HasHrTimer<Self> for Playback { mode: RelativeMode<Monotonic>, field: self.timer, }
}

impl Playback {
    pub(super) fn new(link: Arc<Gate>) -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                link,
                runtime <- kernel::new_spinlock_irq!(Runtime {
                    stream: None,
                    parameters: pcm::Parameters { buffer_frames: 0, period_frames: 1 },
                    clock: clock::Playback::new(RATE as _), closed: false,
                    generation: 0, notified: 0, link_generation: 0,
                }),
                timer <- HrTimer::new(),
                handle <- kernel::new_mutex!(None),
            }),
            GFP_KERNEL,
        )
    }

    pub(super) fn availability(&self) -> (bool, u64) {
        let link = self.link.state.lock();
        (link.enabled, link.generation)
    }

    /// Copy the newest played samples, supplying silence while playback is idle.
    pub(super) fn read(&self, cursor: &mut Cursor, output: &mut [u8]) -> Result {
        if output.len() % FRAME_BYTES != 0 || output.len() / FRAME_BYTES > MAX_READ_FRAMES {
            return Err(EINVAL);
        }
        let link = self.link.state.lock();
        let state = self.runtime.lock();
        if state.closed {
            return Err(ENODEV);
        }
        output.fill(0);
        if !link.enabled || state.link_generation != link.generation {
            return Ok(());
        }
        let Some(stream) = &state.stream else {
            return Ok(());
        };
        let position = state.clock.position(now());
        if cursor.generation != state.generation {
            cursor.generation = state.generation;
            cursor.frames = 0;
        }
        let count = position
            .saturating_sub(cursor.frames)
            .min(state.parameters.buffer_frames as u64)
            .min((output.len() / FRAME_BYTES) as u64) as usize;
        cursor.frames = position;
        if count == 0 {
            return Ok(());
        }
        let offset = if state.clock.running() {
            output.len() - count * FRAME_BYTES
        } else {
            0
        };
        stream.copy_frames(
            position - count as u64,
            &mut output[offset..offset + count * FRAME_BYTES],
        )
    }
}

impl Operations for Playback {
    fn prepare(self: &Arc<Self>, stream: pcm::Stream, parameters: pcm::Parameters) -> Result {
        let mut handle = self.handle.lock();
        let link = self.link.state.lock();
        let mut state = self.runtime.lock();
        if state.closed {
            return Err(ENODEV);
        }
        if !link.enabled {
            return Err(EPIPE);
        }
        if parameters.buffer_frames == 0 || parameters.period_frames == 0 {
            return Err(EINVAL);
        }
        state.generation = state.generation.checked_add(1).ok_or(EOVERFLOW)?;
        state.clock.prepare();
        state.notified = 0;
        state.parameters = parameters;
        state.stream = Some(stream);
        state.link_generation = link.generation;
        drop(state);
        drop(link);
        // Both handles refer to this embedded timer; dropping the old one after
        // start would cancel the new deadline.
        drop(handle.take());
        *handle = Some(self.clone().start(Delta::from_micros(TIMER_INTERVAL_US)));
        Ok(())
    }

    fn stop(&self) {
        let mut handle = self.handle.lock();
        {
            let mut state = self.runtime.lock();
            state.stream = None;
            state.clock.stop(now());
        }
        drop(handle.take());
    }

    fn disconnect(&self) {
        self.runtime.lock().closed = true;
        self.stop();
    }

    fn trigger(&self, command: pcm::Trigger) -> Result {
        let link = self.link.state.lock();
        let mut state = self.runtime.lock();
        if state.closed {
            return Err(ENODEV);
        }
        match command {
            pcm::Trigger::Start | pcm::Trigger::Resume | pcm::Trigger::PauseRelease => {
                if !link.enabled
                    || state.stream.is_none()
                    || state.link_generation != link.generation
                {
                    return Err(EPIPE);
                }
                if matches!(command, pcm::Trigger::Start) {
                    state.notified = 0;
                    state.clock.start(now());
                } else {
                    state.clock.resume(now());
                }
            }
            pcm::Trigger::Stop | pcm::Trigger::Suspend | pcm::Trigger::PausePush => {
                state.clock.stop(now())
            }
        }
        Ok(())
    }

    fn position(&self) -> u64 {
        self.runtime.lock().clock.position(now())
    }

    fn link_time(&self) -> Option<pcm::LinkTime> {
        let state = self.runtime.lock();
        let system = now();
        let frames = state.clock.position(system);
        Some(pcm::LinkTime {
            system: core::time::Duration::from_nanos(system),
            audio: core::time::Duration::new(
                frames / RATE,
                (frames % RATE * 1_000_000_000 / RATE) as _,
            ),
        })
    }
}

impl HrTimerCallback for Playback {
    type Pointer<'a> = Arc<Self>;
    fn run(
        this: ArcBorrow<'_, Self>,
        mut context: HrTimerCallbackContext<'_, Self>,
    ) -> HrTimerRestart {
        let (stream, interrupted) = {
            let link = this.link.state.lock();
            let mut state = this.runtime.lock();
            let position = state.clock.position(now());
            if (!link.enabled || state.link_generation != link.generation)
                && state.stream.is_some()
                && !state.closed
            {
                state.clock.stop(now());
                state.link_generation = link.generation;
                (state.stream.take(), true)
            } else if link.enabled
                && state.clock.running()
                && !state.closed
                && position.saturating_sub(state.notified) >= state.parameters.period_frames as u64
            {
                state.notified = position / state.parameters.period_frames as u64
                    * state.parameters.period_frames as u64;
                (state.stream.clone(), false)
            } else {
                (None, false)
            }
        };
        // Notifications may invoke our stream callbacks. Hold no playback or link lock.
        if let Some(stream) = stream {
            if interrupted {
                let _ = stream.xrun();
            } else {
                let _ = stream.period_elapsed();
            }
        }
        if interrupted {
            return HrTimerRestart::NoRestart;
        }
        context.forward_now(Delta::from_micros(TIMER_INTERVAL_US));
        HrTimerRestart::Restart
    }
}

fn now() -> u64 {
    Instant::<Monotonic>::now().as_nanos() as u64
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kernel::prelude::kunit_tests(rust_castkms_audio_playback)]
mod tests {
    use super::*;
    #[test]
    fn availability_transitions_are_not_lost_between_ticks() -> Result {
        let gate = Gate::new(true)?;
        gate.set_enabled(false);
        gate.set_enabled(true);
        let state = gate.state.lock();
        assert!(state.enabled);
        assert_eq!(state.generation, 2);
        Ok(())
    }

    #[test]
    fn private_read_limits_are_driver_policy() -> Result {
        let playback = Playback::new(Gate::new(true)?)?;
        let mut cursor = Cursor::default();
        assert_eq!(playback.read(&mut cursor, &mut [0; 3]), Err(EINVAL));
        let mut samples = KVec::with_capacity((MAX_READ_FRAMES + 1) * FRAME_BYTES, GFP_KERNEL)?;
        samples.resize((MAX_READ_FRAMES + 1) * FRAME_BYTES, 0xff, GFP_KERNEL)?;
        assert_eq!(playback.read(&mut cursor, &mut samples), Err(EINVAL));
        playback.read(&mut cursor, &mut samples[..FRAME_BYTES])?;
        assert_eq!(&samples[..FRAME_BYTES], &[0; FRAME_BYTES]);
        playback.disconnect();
        assert_eq!(
            playback.read(&mut cursor, &mut samples[..FRAME_BYTES]),
            Err(ENODEV)
        );
        Ok(())
    }

    #[test]
    fn exhausted_link_generations_remain_disabled() -> Result {
        let gate = Gate::new(true)?;
        gate.state.lock().generation = u64::MAX;
        gate.set_enabled(false);
        gate.set_enabled(true);
        assert!(!gate.state.lock().enabled);
        Ok(())
    }
}
