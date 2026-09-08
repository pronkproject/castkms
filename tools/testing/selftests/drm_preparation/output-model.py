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


class ResultLedger:
    def __init__(self, capacity):
        if capacity <= 0:
            raise Rejected()
        self.capacity = capacity
        self.serial = 0
        self.results = {}
        self.closed = False

    def reserve(self):
        if self.closed or len(self.results) >= self.capacity:
            raise Rejected()
        self.serial += 1
        self.results[self.serial] = None
        return self.serial

    def complete(self, use, status):
        # Endpoint close discards delivery, not already admitted native work.
        if self.closed:
            return False
        if use not in self.results or self.results[use] is not None or status is None:
            raise Rejected()
        self.results[use] = status
        return True

    def query(self, use):
        if use not in self.results:
            raise Rejected()
        return self.results[use]

    def acknowledge(self, use):
        if use not in self.results or self.results[use] is None:
            raise Rejected()
        del self.results[use]

    def close(self):
        self.closed = True
        self.results.clear()


@dataclass
class Grant:
    scope: str
    live: bool = True


@dataclass
class CaptureRequest:
    grant: Grant
    claimed: bool = False
    error: object = None


class CaptureRequests:
    """Request credits start before source admission; results outlive access.

    Each operation is serialized. A claimed request stays active until the
    provider acknowledges that access ended, even after cancel or close.
    """

    def __init__(self, capacity):
        self.results = ResultLedger(capacity)
        self.active = {}

    def lookup(self, use):
        if use not in self.active:
            raise Rejected()
        return self.active[use]

    def queue(self, grant):
        if not grant.live:
            raise Rejected()
        use = self.results.reserve()
        self.active[use] = CaptureRequest(grant)
        return use

    def claim(self, use):
        request = self.lookup(use)
        if request.claimed or request.error is not None or not request.grant.live:
            raise Rejected()
        request.claimed = True

    def finish(self, use, status):
        request = self.lookup(use)
        if not request.claimed or not isinstance(status, int) or status > 0:
            raise Rejected()
        if request.error is not None:
            status = request.error
        elif not request.grant.live:
            status = -128  # EKEYREVOKED
        recorded = self.results.complete(use, status)
        del self.active[use]
        return recorded

    def cancel(self, use, error=-125):
        if not isinstance(error, int) or error >= 0:
            raise Rejected()
        request = self.lookup(use)
        if request.error is None:
            request.error = error
        if not request.claimed:
            self.results.complete(use, request.error)
            del self.active[use]

    def revoke(self, grant):
        grant.live = False
        for use, request in list(self.active.items()):
            if request.grant is grant:
                self.cancel(use, -128)

    def close(self):
        self.results.close()
        for use in list(self.active):
            self.cancel(use)


class CaptureRequestTests(unittest.TestCase):
    def test_invalid_or_repeated_transitions_preserve_the_request(self):
        requests = CaptureRequests(1)
        use = requests.queue(Grant("scope"))
        for operation in (lambda: requests.finish(use, 0),
                          lambda: requests.cancel(use, 0),
                          lambda: requests.claim(use + 1)):
            with self.assertRaises(Rejected):
                operation()
            self.assertFalse(requests.active[use].claimed)
            self.assertIsNone(requests.active[use].error)
        requests.claim(use)
        for operation in (lambda: requests.claim(use),
                          lambda: requests.finish(use, 1)):
            with self.assertRaises(Rejected):
                operation()
            self.assertIsNone(requests.results.query(use))
        requests.finish(use, -5)
        for operation in (lambda: requests.finish(use, 0),
                          lambda: requests.cancel(use),
                          lambda: requests.claim(use)):
            with self.assertRaises(Rejected):
                operation()
            self.assertEqual(requests.results.query(use), -5)

    def test_queue_credit_survives_completion_until_ack(self):
        for capacity in (1, 2, 4, 8):
            requests = CaptureRequests(capacity)
            grant = Grant("scope")
            uses = [requests.queue(grant) for _ in range(capacity)]
            with self.assertRaises(Rejected):
                requests.queue(grant)
            for use in uses:
                self.assertFalse(requests.active[use].claimed)
                requests.claim(use)
                requests.finish(use, 0)
                with self.assertRaises(Rejected):
                    requests.queue(grant)
                self.assertEqual(requests.results.query(use), 0)
            for use in uses:
                requests.results.acknowledge(use)
            self.assertGreater(requests.queue(grant), max(uses))

    def test_cancel_waits_for_claimed_access_only(self):
        for claimed in (False, True):
            for error in (-125, -110, -128):
                requests = CaptureRequests(1)
                use = requests.queue(Grant("scope"))
                if claimed:
                    requests.claim(use)
                requests.cancel(use, error)
                if claimed:
                    self.assertIsNone(requests.results.query(use))
                    with self.assertRaises(Rejected):
                        requests.results.acknowledge(use)
                    requests.cancel(use, -5)
                    requests.finish(use, 0)
                self.assertNotIn(use, requests.active)
                self.assertEqual(requests.results.query(use), error)

    def test_revocation_maps_success_without_canceling_native_access(self):
        requests = CaptureRequests(3)
        grant = Grant("old")
        other = Grant("other")
        active, queued, independent = (requests.queue(g) for g in (grant, grant, other))
        requests.claim(active)
        requests.revoke(grant)
        self.assertEqual(requests.results.query(queued), -128)
        self.assertIsNone(requests.results.query(active))
        requests.finish(active, 0)
        self.assertEqual(requests.results.query(active), -128)
        requests.claim(independent)
        requests.finish(independent, -5)
        self.assertEqual(requests.results.query(independent), -5)

    def test_close_discards_delivery_but_retains_active_cleanup(self):
        requests = CaptureRequests(2)
        grant = Grant("scope")
        active, queued = requests.queue(grant), requests.queue(grant)
        requests.claim(active)
        requests.close()
        requests.close()
        self.assertNotIn(queued, requests.active)
        self.assertIn(active, requests.active)
        self.assertFalse(requests.finish(active, 0))
        self.assertFalse(requests.active)
        with self.assertRaises(Rejected):
            requests.queue(grant)


@dataclass
class Allocation:
    scope: str
    pixels: str = "cleared"
    busy: bool = False


@dataclass
class OutputClaim:
    grant: Grant
    allocation: Allocation
    pixels: str
    owner: object
    result: int
    submitted: bool = False
    completed: bool = False
    status: object = None


class OutputModel:
    def __init__(self, result_capacity=4):
        self.results = ResultLedger(result_capacity)

    def allocate(self, grant):
        if not grant.live:
            raise Rejected()
        return Allocation(grant.scope)

    def claim(self, grant, allocation, pixels):
        if not grant.live or allocation.scope != grant.scope or allocation.busy:
            raise Rejected()
        # Reserve delivery capacity before admitting work or owning storage.
        result = self.results.reserve()
        # Claim, not queuing or source-stage permission, authorizes this write.
        allocation.busy = True
        return OutputClaim(grant, allocation, pixels, self, result)

    def revoke(self, grant):
        grant.live = False

    def submit(self, claim):
        if claim.owner is not self or claim.submitted or claim.completed:
            raise Rejected()
        # Pre-revoke authorization survives until the bounded claim resolves.
        claim.submitted = True

    def complete(self, claim, notify=True, *, status=0):
        if claim.owner is not self or not claim.submitted or claim.completed:
            raise Rejected()
        if not isinstance(status, int) or status > 0:
            raise Rejected()
        # Failure does not promise that an exported allocation was untouched.
        claim.allocation.pixels = claim.pixels if status == 0 else "uncertain"
        claim.status = status
        claim.completed = True
        claim.allocation.busy = False
        recorded = self.results.complete(claim.result, status)
        # Lost notification is recoverable through query. Neither query nor
        # acknowledgment is needed to end the allocation's native write use.
        return recorded and claim.grant.live and notify and status == 0


class ResultTests(unittest.TestCase):
    def test_completed_result_holds_credit_until_acknowledged(self):
        ledger = ResultLedger(1)
        first = ledger.reserve()
        self.assertIsNone(ledger.query(first))
        with self.assertRaises(Rejected):
            ledger.acknowledge(first)
        ledger.complete(first, -5)
        for _ in range(3):
            self.assertEqual(ledger.query(first), -5)
            with self.assertRaises(Rejected):
                ledger.reserve()
        with self.assertRaises(Rejected):
            ledger.complete(first, 0)
        ledger.acknowledge(first)
        second = ledger.reserve()
        self.assertNotEqual(first, second)
        with self.assertRaises(Rejected):
            ledger.acknowledge(first)
        self.assertEqual(ledger.results, {second: None})

    def test_result_capacity_is_independent_of_number_of_completions(self):
        for capacity in (1, 2, 4, 8):
            ledger = ResultLedger(capacity)
            for _ in range(20):
                uses = [ledger.reserve() for _ in range(capacity)]
                for use in uses:
                    ledger.complete(use, 0)
                self.assertEqual(len(ledger.results), capacity)
                with self.assertRaises(Rejected):
                    ledger.reserve()
                for use in uses:
                    ledger.acknowledge(use)
                self.assertFalse(ledger.results)

    def test_close_discards_results_without_inventing_completion(self):
        ledger = ResultLedger(2)
        pending = ledger.reserve()
        completed = ledger.reserve()
        ledger.complete(completed, 0)
        ledger.close()
        ledger.close()
        self.assertFalse(ledger.results)
        self.assertFalse(ledger.complete(pending, 0))
        with self.assertRaises(Rejected):
            ledger.reserve()
        with self.assertRaises(Rejected):
            ledger.query(completed)


class OutputTests(unittest.TestCase):
    def test_failed_write_ends_access_without_delivering_valid_pixels(self):
        for notify in (False, True):
            model = OutputModel(result_capacity=1)
            grant = Grant("session")
            allocation = model.allocate(grant)
            claim = model.claim(grant, allocation, "requested")
            model.submit(claim)
            self.assertFalse(model.complete(claim, notify, status=-5))
            self.assertTrue(claim.completed)
            self.assertEqual(claim.status, -5)
            self.assertFalse(allocation.busy)
            self.assertEqual(allocation.pixels, "uncertain")
            self.assertEqual(model.results.query(claim.result), -5)
            model.results.acknowledge(claim.result)
            retry = model.claim(grant, allocation, "replacement")
            with self.assertRaises(Rejected):
                model.complete(claim)
            self.assertTrue(allocation.busy)
            model.submit(retry)
            self.assertTrue(model.complete(retry))

    def test_lost_notification_retains_result_without_owning_allocation(self):
        model = OutputModel(result_capacity=1)
        grant = Grant("session")
        allocation = model.allocate(grant)
        first = model.claim(grant, allocation, "first")
        model.submit(first)
        self.assertFalse(model.complete(first, notify=False))
        self.assertFalse(allocation.busy)
        self.assertEqual(allocation.pixels, "first")
        for _ in range(3):
            self.assertEqual(model.results.query(first.result), 0)
            with self.assertRaises(Rejected):
                model.claim(grant, allocation, "second")
            self.assertFalse(allocation.busy)
        model.results.acknowledge(first.result)
        second = model.claim(grant, allocation, "second")
        with self.assertRaises(Rejected):
            model.results.acknowledge(first.result)
        self.assertEqual(model.results.results, {second.result: None})

    def test_result_endpoint_close_does_not_cancel_an_admitted_write(self):
        model = OutputModel(result_capacity=1)
        grant = Grant("session")
        allocation = model.allocate(grant)
        claim = model.claim(grant, allocation, "authorized")
        model.results.close()
        self.assertTrue(allocation.busy)
        model.submit(claim)
        self.assertFalse(model.complete(claim))
        self.assertFalse(allocation.busy)
        self.assertEqual(allocation.pixels, "authorized")
        self.assertFalse(model.results.results)
        with self.assertRaises(Rejected):
            model.claim(grant, allocation, "new")

    def test_foreign_output_model_cannot_consume_claim(self):
        model = OutputModel()
        other = OutputModel()
        grant = Grant("session")
        allocation = model.allocate(grant)
        claim = model.claim(grant, allocation, "owned")
        with self.assertRaises(Rejected):
            other.submit(claim)
        model.submit(claim)
        with self.assertRaises(Rejected):
            other.complete(claim)
        self.assertTrue(allocation.busy)
        self.assertIsNone(model.results.query(claim.result))
        model.complete(claim)

    def test_allocation_cannot_have_overlapping_write_claims(self):
        model = OutputModel()
        grant = Grant("session")
        allocation = model.allocate(grant)
        first = model.claim(grant, allocation, "first")
        for submitted in (False, True):
            if submitted:
                model.submit(first)
            with self.assertRaises(Rejected):
                model.claim(grant, allocation, "overlap")
            self.assertEqual(allocation.pixels, "cleared")
            self.assertTrue(allocation.busy)
        model.complete(first)
        second = model.claim(grant, allocation, "second")
        # A repeated old completion cannot free a later use of the allocation.
        with self.assertRaises(Rejected):
            model.complete(first)
        self.assertTrue(allocation.busy)
        model.submit(second)
        model.complete(second)
        self.assertFalse(allocation.busy)
        self.assertEqual(allocation.pixels, "second")

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
