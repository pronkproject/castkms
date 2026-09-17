// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::renderer::job::Completion;

#[kunit_tests(rust_castkms_multi_output_offers)]
mod cases {
    use super::*;

    #[test]
    fn eight_workers_accept_one_atomic_scene_cohort() -> Result {
        let display = CastKms::new_constraints(c"castkms-eight-workers", 8)?;
        with_registered_display(&display, |device, _, _, scanout, file| {
            let mut owners = KVec::new();
            let mut endpoints = KVec::new();
            let mut crtcs = KVec::new();
            let mut connectors = KVec::new();
            let mut entries = KVec::new();
            for index in 0..8 {
                let crtc = file.crtc_at(index)?.to_owned_ref();
                let connector = file.connector_at(index)?;
                let owner = owner(&file, crtc.crtc(), &connector)?;
                let endpoint = endpoints::prepared(device, &owner)?;
                endpoint.publish(|_| Ok(()))?;
                let entry = device.constraints_output(crtc.crtc())?
                    .lookup(endpoint.constraints_id()?)?;
                owners.push(owner, GFP_KERNEL)?;
                endpoints.push(endpoint, GFP_KERNEL)?;
                crtcs.push(crtc, GFP_KERNEL)?;
                connectors.push(connector, GFP_KERNEL)?;
                entries.push(entry, GFP_KERNEL)?;
            }
            device.atomic_update(|mut state| {
                for index in 0..8 {
                    let connectors = [&*connectors[index]];
                    let target = CrtcScanout {
                        mode: scanout.mode,
                        framebuffer: scanout.framebuffer,
                        connectors: &connectors,
                        position: (0, 0),
                    };
                    state.as_mut().set_crtc_config(crtcs[index].crtc(), Some(&target))?;
                    state.add_crtc_state(crtcs[index].crtc())?
                        .set_constraints(&entries[index])?;
                }
                Ok(())
            })?;
            for round in 0..4 {
                let mut ids = KVec::new();
                for (index, endpoint) in endpoints.iter().enumerate() {
                    check(device.constraints_output(crtcs[index].crtc())?
                        .selected().id() == entries[index].id())?;
                    let pending = endpoint.begin_source(1)?;
                    check(pending.constraints_id() == entries[index].id())?;
                    ids.push(pending.id(), GFP_KERNEL)?;
                    pending.publish(|| ())?;
                }
                for (endpoint, id) in endpoints.iter().zip(ids) {
                    endpoint.release_source(id, Completion::Cpu)?;
                }
                if round < 3 {
                    device.atomic_update(|state| {
                        for crtc in &crtcs {
                            state.add_crtc_state(crtc.crtc())?;
                        }
                        Ok(())
                    })?;
                    // Complete-scene validation adds planes but does not resubmit them.
                    for endpoint in &endpoints {
                        check(endpoint.begin_source(1).err() == Some(ENODATA))?;
                    }
                    device.atomic_update(|mut state| {
                        for index in 0..8 {
                            let connectors = [&*connectors[index]];
                            let target = CrtcScanout {
                                mode: scanout.mode,
                                framebuffer: scanout.framebuffer,
                                connectors: &connectors,
                                position: (0, 0),
                            };
                            state.as_mut().set_crtc_config(crtcs[index].crtc(), Some(&target))?;
                        }
                        Ok(())
                    })?;
                }
            }
            Ok(())
        })
    }

    #[test]
    fn stale_last_offer_leaves_every_selection_unchanged() -> Result {
        let display = CastKms::new_constraints(c"castkms-offer-cohort-reject", 8)?;
        with_registered_display(&display, |device, _, _, _, file| {
            let mut owners = KVec::new();
            let mut endpoints = KVec::new();
            let mut crtcs = KVec::new();
            let mut entries = KVec::new();
            for index in 0..8 {
                let crtc = file.crtc_at(index)?.to_owned_ref();
                let connector = file.connector_at(index)?;
                let owner = owner(&file, crtc.crtc(), &connector)?;
                let endpoint = endpoints::prepared(device, &owner)?;
                endpoint.publish(|_| Ok(()))?;
                entries.push(device.constraints_output(crtc.crtc())?
                    .lookup(endpoint.constraints_id()?)?, GFP_KERNEL)?;
                owners.push(owner, GFP_KERNEL)?;
                endpoints.push(endpoint, GFP_KERNEL)?;
                crtcs.push(crtc, GFP_KERNEL)?;
            }
            endpoints[7].withdraw()?;
            check(device.atomic_update(|state| {
                for index in 0..8 {
                    state.add_crtc_state(crtcs[index].crtc())?
                        .set_constraints(&entries[index])?;
                }
                Ok(())
            }) == Err(ESTALE))?;
            for crtc in &crtcs {
                let output = device.constraints_output(crtc.crtc())?;
                check(output.selected().id() == output.default_entry().id())?;
            }
            Ok(())
        })
    }
}
