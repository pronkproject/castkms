// SPDX-License-Identifier: GPL-2.0-only

//! Execution descriptions belong to one device and outlive their replacement handle.

use super::*;
use kernel::{
    bindings,
    drm::kms::connector::AsRawConnector, //
};

fn bytes(fixture: &Fixture) -> Result<[u8; 16]> {
    let connector = fixture.drm.connector()?;
    // SAFETY: The private initialized fixture excludes teardown and property replacement.
    let id = unsafe {
        let properties = &*(*connector.as_raw()).base.properties;
        (0..properties.count as usize)
            .find(|&index| {
                CStr::from_char_ptr((*properties.properties[index]).name.as_ptr())
                    == c"CASTKMS_EXECUTION"
            })
            .map(|index| properties.values[index] as u32)
            .ok_or(ENOENT)?
    };
    // SAFETY: The private device retains initialized mode configuration and its object registry.
    let raw = unsafe { bindings::drm_property_lookup_blob((*connector.as_raw()).dev, id) };
    if raw.is_null() {
        return Err(ENOENT);
    }
    // SAFETY: Lookup retains immutable storage; validate its length before reading the bytes.
    let result = unsafe {
        if (*raw).length == 16 {
            Ok(*(*raw).data.cast::<[u8; 16]>())
        } else {
            Err(EINVAL)
        }
    };
    // SAFETY: Release only the lookup reference, outside driver and object-ID locks.
    unsafe { bindings::drm_property_blob_put(raw) };
    result
}

#[kunit_tests(rust_castkms_execution_publication)]
mod cases {
    use super::*;

    #[test]
    fn explicit_close_keeps_the_installed_description_valid() -> Result {
        let fixture = Fixture::new()?;
        let expected = crate::execution::Description {
            profile: crate::execution::Profile::HostV1,
            generation: 1,
        };
        check(fixture.drm.device().execution.describe() == expected)?;
        let before = bytes(&fixture)?;
        check(u32::from_ne_bytes(before[0..4].try_into().map_err(|_| EINVAL)?) == 1)?;
        check(u32::from_ne_bytes(before[4..8].try_into().map_err(|_| EINVAL)?) == 1)?;
        check(u64::from_ne_bytes(before[8..16].try_into().map_err(|_| EINVAL)?) == 1)?;
        fixture.state.close();
        fixture.state.close();
        check(bytes(&fixture)? == before)?;
        check(fixture.drm.device().execution.describe() == expected)
    }

    #[test]
    fn closing_one_device_does_not_remove_another_description() -> Result {
        let first = Fixture::new()?;
        let second = Fixture::new_named(c"castkms-execution-other")?;
        let expected = second.drm.device().execution.describe();
        let encoded = bytes(&second)?;
        first.state.close();
        check(second.drm.device().execution.describe() == expected)?;
        check(bytes(&second)? == encoded)?;
        let fb = second.framebuffer(provenance::Provenance::from_snapshot(None))?;
        second.select(&fb, false, 0)?;
        check(bytes(&second)? == encoded)
    }
}
