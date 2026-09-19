// SPDX-License-Identifier: GPL-2.0-only

//! Transport-independent offer names retain configuration identity, not authorization.

use super::*;
use crate::capture::negotiation::Negotiation;
use kernel::drm::{capture::RequestedLayout, fourcc};

#[kunit_tests(rust_castkms_capture_negotiation)]
mod cases {
    use super::*;

    #[test]
    fn queries_and_content_updates_preserve_the_offer() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let mut negotiation = Negotiation::new(grantor.capture());
        check(matches!(negotiation.open(0, 1), Err(EINVAL)))?;
        check(matches!(negotiation.open(1, 1), Err(ESTALE)))?;
        let offer = negotiation.describe()?;
        let id = offer.id();
        check(id == 1)?;
        check(offer.description().dimensions() == [640, 480])?;
        for _ in 0..32 {
            check(negotiation.describe()?.id() == id)?;
        }
        fixture.select(&fb, false, 0)?;
        check(negotiation.describe()?.id() == id)?;
        check(matches!(fixture.drm.device().host.current(), Err(EAGAIN)))?;
        check(matches!(negotiation.open(id + 1, 1), Err(ESTALE)))?;
        check(matches!(negotiation.open(id, 0), Err(EINVAL)))?;
        let mut queue = negotiation.open(id, 1)?;
        queue.queue(1)?;
        fixture.drm.device().host.current()?.flush_for_test();
        check(queue.advance() == 1)?;
        queue.dequeue(|completion| {
            completion.result.map(|_| ())
        })?;
        let _second = negotiation.open(id, 1)?;
        Ok(())
    }

    #[test]
    fn a_modeset_requires_an_explicitly_queried_replacement() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let mut negotiation = Negotiation::new(grantor.capture());
        let old = negotiation.describe()?.id();
        fixture.drm.update(|transaction| {
            transaction
                .add_crtc_state(fixture.drm.crtc()?)?
                .set_mode_changed(true);
            Ok(())
        })?;
        check(matches!(negotiation.open(old, 1), Err(ESTALE)))?;
        let fresh = negotiation.describe()?.id();
        check(fresh == old + 1)?;
        check(matches!(negotiation.open(old, 1), Err(ESTALE)))?;
        let _queue = negotiation.open(fresh, 1)?;
        Ok(())
    }

    #[test]
    fn host_offers_only_its_exact_linear_layout() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let mut negotiation = Negotiation::new(grantor.capture());
        let default = negotiation.describe()?.id();
        let host = RequestedLayout::exact(fourcc::XRGB8888, fourcc::FORMAT_MOD_LINEAR)?;
        check(negotiation.describe_layout(host)?.id() == default)?;
        let other = RequestedLayout::exact(fourcc::ARGB8888, fourcc::FORMAT_MOD_LINEAR)?;
        check(matches!(negotiation.describe_layout(other), Err(EOPNOTSUPP)))?;
        check(negotiation.open(default, 1).is_ok())
    }

    #[test]
    fn a_retained_name_does_not_keep_the_grant_alive() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let mut negotiation = Negotiation::new(grantor.capture());
        let id = negotiation.describe()?.id();
        drop(grantor);
        check(matches!(negotiation.describe(), Err(EKEYREVOKED)))?;
        check(matches!(negotiation.open(id, 1), Err(EKEYREVOKED)))?;
        check(matches!(fixture.drm.device().host.current(), Err(EAGAIN)))
    }
}
