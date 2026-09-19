// SPDX-License-Identifier: GPL-2.0-only

//! Attachment-owned ALSA registration and exclusive private tap admission.

use super::playback::{Gate, Playback};
use super::tap::Tap;
use crate::{
    CastKms,
    Driver, //
};
use kernel::{
    drm::{
        device::Registered,
        kms::connector::Edid,
        Device, //
    },
    module::this_module,
    prelude::*,
    sound::pcm::{
        Config,
        DisplayAudio,
        Format,
        Identity,
        Registration, //
    },
    str::CString,
    sync::{
        Arc,
        Mutex, //
    }, //
};

struct State {
    closed: bool,
    tap: Option<Arc<Tap>>,
}

/// Retained capture source, not an owner of the sound-card registration.
#[pin_data]
pub(crate) struct Source {
    #[pin]
    state: Mutex<State>,
    playback: Arc<Playback>,
}

impl Source {
    pub(super) fn open(&self) -> Result<Arc<Tap>> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(ENOTCONN);
        }
        if state
            .tap
            .as_ref()
            .is_some_and(|tap| tap.terminal().is_none())
        {
            return Err(EBUSY);
        }
        let tap = Tap::new(self.playback.clone())?;
        state.tap = Some(tap.clone());
        Ok(tap)
    }

    fn close(&self) {
        let tap = {
            let mut state = self.state.lock();
            state.closed = true;
            state.tap.take()
        };
        if let Some(tap) = tap {
            tap.terminate(ENOTCONN);
        }
    }
}

/// Dropping the monitor attachment disconnects ALSA even while capture handles survive.
pub(crate) struct Attachment {
    pub(crate) source: Arc<Source>,
    eld: [u8; kernel::bindings::MAX_ELD_BYTES as usize],
    eld_len: usize,
    _card: Registration<Playback>,
}

impl Attachment {
    pub(crate) fn matches_eld(&self, edid: &Edid) -> Result<bool> {
        Ok(edid
            .eld(false)?
            .is_some_and(|eld| &self.eld[..self.eld_len] == eld.as_bytes()))
    }

    pub(crate) fn new(
        device: &Device<Driver, Registered>,
        edid: &Edid,
        index: usize,
        link: Arc<Gate>,
    ) -> Result<Option<Self>> {
        let Some(eld) = edid.eld(false)? else {
            return Ok(None);
        };
        let mut eld_bytes = [0; kernel::bindings::MAX_ELD_BYTES as usize];
        let eld_len = eld.as_bytes().len();
        if eld_len > eld_bytes.len() {
            return Err(EOVERFLOW);
        }
        eld_bytes[..eld_len].copy_from_slice(eld.as_bytes());
        let id = CString::try_from_fmt(fmt!("CastKMS{index}"))?;
        let mut display_name = [0; 17];
        let monitor_name = eld.monitor_name(&mut display_name);
        let fallback = CString::try_from_fmt(fmt!("CastKMS HDMI {index}"))?;
        let name: &kernel::str::CStr = if monitor_name.to_bytes().is_empty() {
            &fallback
        } else {
            monitor_name
        };
        let playback = Playback::new(link)?;
        let card = Registration::new(
            device.as_ref().as_ref(),
            this_module::<CastKms>(),
            Identity {
                id: &id,
                driver: c"castkms",
                name,
            },
            Config {
                format: Format::S16Le,
                rate: super::RATE as _,
                channels: 2,
                buffer_bytes_max: (super::BUFFER_FRAMES * super::FRAME_BYTES) as _,
                period_bytes_min: 256,
                period_bytes_max: 65_536,
                periods_min: 2,
                periods_max: 32,
                pause: true,
                link_timestamps: true,
            },
            Some(DisplayAudio {
                eld: eld.as_bytes(),
                jack_name: c"HDMI/DP,pcm=0",
            }),
            playback.clone(),
        )?;
        let source = Arc::pin_init(
            pin_init!(Source {
                state <- kernel::new_mutex!(State { closed: false, tap: None }),
                playback,
            }),
            GFP_KERNEL,
        )?;
        Ok(Some(Self {
            source,
            eld: eld_bytes,
            eld_len,
            _card: card,
        }))
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        self.source.close();
    }
}
