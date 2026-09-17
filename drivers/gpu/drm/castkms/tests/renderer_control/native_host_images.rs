// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::{host_compositor::{budget::Budget, compose, layout::Layout, pool::Pool}, image_access};

#[kunit_tests(rust_castkms_native_host_images)]
mod cases {
    use super::*;

    #[test]
    fn renderer_selection_excludes_host_images() -> Result {
        let display = CastKms::new_constraints(c"castkms-native-host-images", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let grantor = capture_grant(&file, crtc, connector)?;
            let capture = grantor.capture();
            let description = capture.describe_stream()?;
            let stream = description.create_stream(1)?;
            stream.check_current()?;
            let pool = Pool::new(device, &Budget::new()?, Layout::new(640, 480)?)?;
            let image = compose::current(&crtc.display.output, &pool)?.ok_or(ENODATA)?;
            owner.access().with_current(|current| {
                image_access::Current::new(current)?.check_image(&image)
            })?;
            let endpoint = endpoints::prepared(device, &owner)?;
            endpoint.publish(|_| Ok(()))?;
            let output = device.constraints_output(crtc)?;
            let entry = output.lookup(endpoint.constraints_id()?)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;
            // The framebuffer and geometry are unchanged and remain CPU-readable. Neither
            // fact authorizes HOST delivery while the accepted backend is a renderer.
            check(owner.access().with_current(|current| {
                image_access::Current::new(current)?.check_image(&image)
            }) == Err(EOPNOTSUPP))?;
            check(capture.describe_stream().err() == Some(EOPNOTSUPP))?;
            check(description.create_stream(1).err() == Some(EOPNOTSUPP))?;
            check(stream.check_current() == Err(EOPNOTSUPP))?;
            check(compose::current(&crtc.display.output, &pool).err() == Some(EOPNOTSUPP))?;
            let provider = crtc.display.constraints.as_ref().ok_or(EINVAL)?;
            device.atomic_update(|state| {
                state.add_crtc_state(crtc)?.set_constraints(provider.initial())
            })?;
            capture.describe_stream()?.create_stream(1)?.check_current()?;
            let fresh = compose::current(&crtc.display.output, &pool)?.ok_or(ENODATA)?;
            check(fresh.content_serial() != image.content_serial())?;
            owner.access().with_current(|current| {
                image_access::Current::new(current)?.check_image(&fresh)
            })?;
            Ok(())
        })
    }
}
