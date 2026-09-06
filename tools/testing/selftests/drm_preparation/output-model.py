#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0
"""Authorization lifetime model for non-revocable exported output storage.

Allocation identity survives stream and grant teardown. Pixel writes occur
at native completion, not when a completion notification is delivered.
"""

from dataclasses import dataclass
import unittest


class Rejected(Exception):
    pass


@dataclass
class Grant:
    scope: str
    live: bool = True


@dataclass
class Allocation:
    scope: str
    pixels: str = "cleared"


@dataclass
class OutputClaim:
    grant: Grant
    allocation: Allocation
    pixels: str
    submitted: bool = False
    completed: bool = False


class OutputModel:
    def allocate(self, grant):
        if not grant.live:
            raise Rejected()
        return Allocation(grant.scope)

    def claim(self, grant, allocation, pixels):
        if not grant.live or allocation.scope != grant.scope:
            raise Rejected()
        # Claim, not queuing or source-stage permission, authorizes this write.
        return OutputClaim(grant, allocation, pixels)

    def revoke(self, grant):
        grant.live = False

    def submit(self, claim):
        if claim.submitted or claim.completed:
            raise Rejected()
        # Pre-revoke authorization survives until the bounded claim resolves.
        claim.submitted = True

    def complete(self, claim):
        if not claim.submitted or claim.completed:
            raise Rejected()
        claim.allocation.pixels = claim.pixels
        claim.completed = True
        return claim.grant.live  # Delivery eligibility, not write revocation.


class OutputTests(unittest.TestCase):
    def test_old_descriptor_never_receives_new_scope_pixels(self):
        model = OutputModel()
        old = Grant("old authorization domain")
        retained_descriptor = model.allocate(old)
        first = model.claim(old, retained_descriptor, "old pixels")
        model.submit(first)
        model.complete(first)
        model.revoke(old)
        new = Grant("new authorization domain")
        with self.assertRaises(Rejected):
            model.claim(new, retained_descriptor, "new pixels")
        fresh = model.allocate(new)
        second = model.claim(new, fresh, "new pixels")
        model.submit(second)
        model.complete(second)
        self.assertEqual(retained_descriptor.pixels, "old pixels")
        self.assertEqual(fresh.pixels, "new pixels")

    def test_revocation_does_not_undo_an_authorized_write(self):
        model = OutputModel()
        grant = Grant("session")
        allocation = model.allocate(grant)
        claim = model.claim(grant, allocation, "authorized pixels")
        model.submit(claim)
        model.revoke(grant)
        delivered = model.complete(claim)
        self.assertFalse(delivered)
        self.assertEqual(allocation.pixels, "authorized pixels")

    def test_pre_revoke_claim_may_submit_after_revocation(self):
        model = OutputModel()
        grant = Grant("session")
        allocation = model.allocate(grant)
        claim = model.claim(grant, allocation, "authorized pixels")
        model.revoke(grant)
        with self.assertRaises(Rejected):
            model.claim(grant, allocation, "unauthorized pixels")
        model.submit(claim)
        self.assertFalse(model.complete(claim))
        self.assertEqual(allocation.pixels, "authorized pixels")

    def test_same_scope_reuse_does_not_require_per_frame_allocation(self):
        model = OutputModel()
        grant = Grant("session")
        allocation = model.allocate(grant)
        for pixels in ("first", "second", "third"):
            claim = model.claim(grant, allocation, pixels)
            model.submit(claim)
            self.assertTrue(model.complete(claim))
            self.assertEqual(allocation.pixels, pixels)


if __name__ == "__main__":
    unittest.main()
