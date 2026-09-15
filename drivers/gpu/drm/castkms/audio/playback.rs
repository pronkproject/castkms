// SPDX-License-Identifier: GPL-2.0-only

//! Timer-driven CastKMS playback.

mod clock;

use super::RATE;
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

struct Runtime {
    stream: Option<pcm::Stream>,
    parameters: pcm::Parameters,
    clock: clock::Playback,
    closed: bool,
    notified: u64,
}

#[pin_data]
pub(crate) struct Playback {
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
    pub(super) fn new() -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                runtime <- kernel::new_spinlock_irq!(Runtime {
                    stream: None,
                    parameters: pcm::Parameters { buffer_frames: 0, period_frames: 1 },
                    clock: clock::Playback::new(RATE as _), closed: false,
                    notified: 0,
                }),
                timer <- HrTimer::new(),
                handle <- kernel::new_mutex!(None),
            }),
            GFP_KERNEL,
        )
    }
}

impl Operations for Playback {
    fn prepare(self: &Arc<Self>, stream: pcm::Stream, parameters: pcm::Parameters) -> Result {
        let mut handle = self.handle.lock();
        let mut state = self.runtime.lock();
        if state.closed {
            return Err(ENODEV);
        }
        if parameters.buffer_frames == 0 || parameters.period_frames == 0 {
            return Err(EINVAL);
        }
        state.clock.prepare();
        state.notified = 0;
        state.parameters = parameters;
        state.stream = Some(stream);
        drop(state);
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
        let mut state = self.runtime.lock();
        if state.closed {
            return Err(ENODEV);
        }
        match command {
            pcm::Trigger::Start | pcm::Trigger::Resume | pcm::Trigger::PauseRelease => {
                if state.stream.is_none() {
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
        let stream = {
            let mut state = this.runtime.lock();
            let position = state.clock.position(now());
            if state.clock.running()
                && !state.closed
                && position.saturating_sub(state.notified) >= state.parameters.period_frames as u64
            {
                state.notified = position / state.parameters.period_frames as u64
                    * state.parameters.period_frames as u64;
                state.stream.clone()
            } else {
                None
            }
        };
        // Notifications may invoke our stream callbacks. Hold no playback lock.
        if let Some(stream) = stream {
            let _ = stream.period_elapsed();
        }
        context.forward_now(Delta::from_micros(TIMER_INTERVAL_US));
        HrTimerRestart::Restart
    }
}

fn now() -> u64 {
    Instant::<Monotonic>::now().as_nanos() as u64
}
