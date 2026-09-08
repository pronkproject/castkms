// SPDX-License-Identifier: GPL-2.0-only

use super::*;

#[kunit_tests(rust_castkms_scene)]
mod cases {
    use super::*;

    #[test]
    fn content_serial_starts_nonzero() -> Result {
        let first = ContentSerial::for_update(None, true)?;
        assert_eq!(first.unwrap().0.get(), 1);
        assert_eq!(ContentSerial::for_update(first, true)?.unwrap().0.get(), 2);
        Ok(())
    }

    #[test]
    fn rechecking_a_candidate_does_not_consume_serials() -> Result {
        let accepted = ContentSerial::for_update(None, true)?;
        let tested = ContentSerial::for_update(accepted, true)?;
        let retried = ContentSerial::for_update(accepted, true)?;
        assert_eq!(tested, retried);
        assert_eq!(accepted.unwrap().0.get(), 1);
        assert_eq!(
            ContentSerial::for_update(retried, true)?.unwrap().0.get(),
            3
        );
        Ok(())
    }

    #[test]
    fn blanking_preserves_sequence_for_reactivation() -> Result {
        assert_eq!(ContentSerial::for_update(None, false)?, None);
        let first = ContentSerial::for_update(None, true)?;
        let blank = ContentSerial::for_update(first, false)?;
        assert_eq!(blank, first);
        assert_eq!(ContentSerial::for_update(blank, true)?.unwrap().0.get(), 2);
        Ok(())
    }

    #[test]
    fn content_serial_exhaustion_does_not_wrap() {
        let last = ContentSerial(NonZeroU64::MAX);
        assert_eq!(ContentSerial::for_update(Some(last), true), Err(EOVERFLOW));
        assert_eq!(ContentSerial::for_update(Some(last), false), Ok(Some(last)));
        assert_eq!(last.0.get(), u64::MAX);
    }
}
