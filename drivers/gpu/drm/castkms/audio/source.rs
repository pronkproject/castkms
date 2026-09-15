// SPDX-License-Identifier: GPL-2.0-only

//! Attachment-owned ALSA registration.

use super::playback::{Gate, Playback};
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
    sync::Arc, //
};

/// Dropping the monitor attachment disconnects ALSA even while capture handles survive.
pub(crate) struct Attachment {
    _card: Registration<Playback>,
}

impl Attachment {
    pub(crate) fn new(
        device: &Device<Driver, Registered>,
        edid: &Edid,
        index: usize,
        link: Arc<Gate>,
    ) -> Result<Option<Self>> {
        let Some(eld) = edid.eld(false)? else {
            return Ok(None);
        };
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
        Ok(Some(Self { _card: card }))
    }
}
