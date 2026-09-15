// SPDX-License-Identifier: GPL-2.0-only

//! Pending capabilities remain separate from execution and source authority.

use super::*;
use crate::{
    execution::capabilities::{ColorLimits, Format, GeometryLimits, Limits, Profile},
    renderer::{
        candidate::Candidate,
        permission::{Owner, Permission},
    },
};
use kernel::drm::kms::testing::MasterFile;

fn owner(fixture: &Fixture, file: &MasterFile<'_, Driver>) -> Result<Owner> {
    let master = file.file().master_snapshot().ok_or(EINVAL)?;
    let permission = {
        let guard = master.master().lock_current().ok_or(EACCES)?;
        Permission::new(&guard, fixture.drm.crtc()?, fixture.drm.connector()?)?
    };
    Owner::new(permission)
}

fn enable(fixture: &Fixture) -> Result {
    let image = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
    fixture.select(&image, false, 0)
}

pub(super) fn profile() -> Result<Profile> {
    let mut formats = KVec::new();
    // Deliberately not compatible with the fixture: proposing must not migrate it.
    formats.push(
        Format {
            fourcc: drm::fourcc::XRGB8888,
            modifier: Some(0x0100_0000_0000_0001),
            planes: 1,
            native: false,
            imported: true,
            pitch_alignment: 128,
            offset_alignment: 4096,
            max_pitch: 65536,
        },
        GFP_KERNEL,
    )?;
    Profile::new(
        Limits {
            geometry: GeometryLimits {
                output: [16384; 2],
                source: [16384; 2],
                crop: true,
                fractional: true,
                position: true,
                scale: true,
                min_scale: 1 << 12,
                max_scale: 1 << 20,
            },
            color: ColorLimits {
                operations: 16,
                srgb: true,
                plane_matrix: true,
                output_matrix: true,
                lut_entries: 256,
                yuv_encodings: [true; 3],
                yuv_ranges: [true; 2],
            },
            layers: 24,
            roles: [1, 22, 1],
        },
        formats,
    )
}

#[kunit_tests(rust_castkms_renderer_proposals)]
mod cases {
    use super::*;

    #[test]
    fn proposal_does_not_restrict_animation_or_change_execution() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        enable(&fixture)?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
        let before = fixture.drm.device().execution.describe();
        let proposal = candidate.propose_profile(profile()?)?;
        check(proposal.describe().expected == before)?;
        check(proposal.describe().generation == 2)?;
        let observed = fixture
            .drm
            .device()
            .execution
            .pending_profile()
            .ok_or(EINVAL)?;
        check(Arc::ptr_eq(&observed.profile, &proposal.describe().profile))?;
        for _ in 0..8 {
            enable(&fixture)?;
            proposal.validate()?;
            check(fixture.drm.device().execution.describe() == before)?;
            fixture.drm.device().execution.check_host()?;
        }
        check(matches!(candidate.propose_profile(profile()?), Err(EBUSY)))?;
        proposal.cancel();
        check(proposal.validate() == Err(ESTALE))?;
        let replacement = candidate.propose_profile(profile()?)?;
        check(replacement.describe().generation > proposal.describe().generation)?;
        drop(proposal);
        drop(candidate);
        replacement.validate()?;
        drop(replacement);
        check(fixture.drm.device().execution.pending_profile().is_none())?;
        // Historical metadata outlives registration without retaining its slot.
        check(observed.profile.formats().len() == 1)
    }

    #[test]
    fn revoked_proposals_remain_cancellable() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        enable(&fixture)?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
        let proposal = candidate.propose_profile(profile()?)?;
        owner.revoke();
        check(proposal.validate() == Err(EKEYREVOKED))?;
        check(matches!(
            candidate.propose_profile(profile()?),
            Err(EKEYREVOKED)
        ))?;
        proposal.cancel();
        check(fixture.drm.device().execution.pending_profile().is_none())
    }

    #[test]
    fn proposal_cancellation_lifts_its_installed_gate() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        enable(&fixture)?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
        let proposal = candidate.propose_profile(profile()?)?;
        fixture.drm.device().validation.gate_for_test(
            0,
            proposal.describe().transition,
            crate::execution::validation::Contract::Renderer(proposal.describe().profile.clone()),
        )?;
        check(enable(&fixture) == Err(EOPNOTSUPP))?;
        proposal.cancel();
        enable(&fixture)?;
        let replacement = candidate.propose_profile(profile()?)?;
        check(replacement.describe().transition != proposal.describe().transition)?;
        drop(proposal);
        replacement.validate()
    }

    #[test]
    fn permission_revocation_lifts_a_gate_without_dropping_the_proposal() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        enable(&fixture)?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
        let proposal = candidate.propose_profile(profile()?)?;
        fixture.drm.device().validation.gate_for_test(
            0,
            proposal.describe().transition,
            crate::execution::validation::Contract::Renderer(proposal.describe().profile.clone()),
        )?;
        check(enable(&fixture) == Err(EOPNOTSUPP))?;
        owner.revoke();
        check(proposal.validate() == Err(EKEYREVOKED))?;
        enable(&fixture)
    }

    #[test]
    fn canceled_candidate_cannot_remove_replacement_profile() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        enable(&fixture)?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let first = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
        let old = first.propose_profile(profile()?)?;
        first.cancel();
        check(fixture.drm.device().execution.pending_profile().is_none())?;
        let second = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
        let new = second.propose_profile(profile()?)?;
        first.cancel();
        drop(old);
        drop(first);
        new.validate()
    }

    #[test]
    fn closing_output_removes_pending_metadata() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        enable(&fixture)?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
        let proposal = candidate.propose_profile(profile()?)?;
        fixture.drm.device().execution.close();
        check(fixture.drm.device().execution.pending_profile().is_none())?;
        check(proposal.validate() == Err(ENODEV))?;
        drop(proposal);
        Ok(())
    }

    #[test]
    fn changed_configuration_does_not_leave_an_obsolete_proposal_blocking_startup() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        enable(&fixture)?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let first = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
        let old = first.propose_profile(profile()?)?;
        fixture
            .drm
            .update(|mut state| state.as_mut().set_crtc_config(fixture.drm.crtc()?, None))?;
        enable(&fixture)?;
        check(old.validate() == Err(ESTALE))?;
        let replacement = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
        check(fixture.drm.device().execution.pending_profile().is_none())?;
        let new = replacement.propose_profile(profile()?)?;
        drop(old);
        drop(first);
        new.validate()
    }

    #[test]
    fn equal_generations_on_distinct_outputs_do_not_alias() -> Result {
        let first = Fixture::new()?;
        let second = Fixture::new_named(c"castkms-proposal-other")?;
        let _first_connector = first.drm.publish_connector_identity()?;
        let _second_connector = second.drm.publish_connector_identity()?;
        enable(&first)?;
        enable(&second)?;
        let first_file = first.drm.master_file()?;
        let second_file = second.drm.master_file()?;
        let first_owner = owner(&first, &first_file)?;
        let second_owner = owner(&second, &second_file)?;
        let first_candidate = Arc::new(Candidate::begin(first_owner.access())?, GFP_KERNEL)?;
        let second_candidate = Arc::new(Candidate::begin(second_owner.access())?, GFP_KERNEL)?;
        let first_proposal = first_candidate.propose_profile(profile()?)?;
        let second_proposal = second_candidate.propose_profile(profile()?)?;
        check(first_proposal.describe().generation == second_proposal.describe().generation)?;
        drop(first_proposal);
        check(first.drm.device().execution.pending_profile().is_none())?;
        second_proposal.validate()
    }
}
