// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use kernel::prelude::*;

#[kunit_tests(rust_castkms_provenance)]
mod tests {
    use super::*;

    #[test]
    fn creator_survives_handoff_without_adoption() {
        let provenance = Provenance {
            origin: Origin::Creator(1),
        };
        assert_eq!(provenance.owner(Some(&2)), Some(&1));
        assert_eq!(
            provenance.committed_owner(Some(&2), Selection::RetainedFramebuffer),
            Some(&1)
        );
        assert_eq!(provenance.owner(None), Some(&1));
    }

    #[test]
    fn association_requires_matching_current_identity() {
        let provenance = Provenance {
            origin: Origin::Associated(1),
        };
        assert_eq!(provenance.owner(Some(&1)), Some(&1));
        assert_eq!(provenance.owner(Some(&2)), None);
        assert_eq!(provenance.owner(None), None);
    }

    #[test]
    fn different_selection_adopts_without_mutating_creation_evidence() {
        let provenance = Provenance {
            origin: Origin::Creator(1),
        };
        assert_eq!(
            provenance.committed_owner(Some(&2), Selection::DifferentFramebuffer),
            Some(&2)
        );
        assert_eq!(provenance.owner(Some(&2)), Some(&1));
        assert_eq!(
            provenance.committed_owner(None, Selection::DifferentFramebuffer),
            Some(&1)
        );
    }

    #[test]
    fn unknown_creation_is_not_implicitly_owned() {
        let provenance = Provenance {
            origin: Origin::<u32>::Unknown,
        };
        assert_eq!(
            provenance.committed_owner(Some(&2), Selection::RetainedFramebuffer),
            None
        );
        assert_eq!(
            provenance.committed_owner(Some(&2), Selection::DifferentFramebuffer),
            Some(&2)
        );
        assert_eq!(
            provenance.committed_owner(None, Selection::DifferentFramebuffer),
            None
        );
    }
}
