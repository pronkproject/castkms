#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0
"""Commit-attempt ownership model, not a kernel locking implementation.

The source/worker model supplies admission and native completion. This layer
makes reservation, cancellation and acceptance separate serialized decisions.
No host preparation wait or native fence wait takes place inside a decision.
"""

from dataclasses import dataclass
from itertools import permutations
from pathlib import Path
import runpy
import unittest


base = runpy.run_path(str(Path(__file__).with_name("preparation-model.py")))
Model = base["Model"]
Rejected = base["Rejected"]


@dataclass
class Attempt:
    ticket: int
    scope: dict
    fences: frozenset
    state: str = "RESERVED"


class ReservationModel(Model):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.attempts = {}
        self.reservations = {}

    def reserve(self, ticket_id, *, allocation_failure=False):
        self.accept(ticket_id, test_only=True)
        self.require(ticket_id not in self.reservations)
        ticket = self.tickets[ticket_id]
        self.require(all(output not in self.pending for output in ticket.scope))
        self.require(self.ready(ticket_id))
        # Allocation and completion collection precede publishing an attempt.
        self.require(not allocation_failure)
        key = self.identity()
        self.attempts[key] = Attempt(ticket_id, dict(ticket.scope), ticket.fences)
        self.reservations[ticket_id] = key
        for scene in ticket.scope.values():
            self.scenes[scene].seals.add(("attempt", key))
        return key

    def abort(self, attempt_id):
        attempt = self.attempts[attempt_id]
        self.require(attempt.state == "RESERVED")
        self.require(self.reservations.get(attempt.ticket) == attempt_id)
        del self.reservations[attempt.ticket]
        attempt.state = "ABORTED"
        for scene in attempt.scope.values():
            self.scenes[scene].seals.discard(("attempt", attempt_id))

    def accept(self, ticket_id, *, test_only=False, failure=False):
        # A reserved ticket cannot bypass its attempt. TEST_ONLY consumes none
        # of its ownership and does not require native readers to have finished.
        self.require(test_only or ticket_id not in self.reservations)
        return super().accept(ticket_id, test_only=test_only, failure=failure)

    def accept_attempt(self, attempt_id, *, failure=False):
        attempt = self.attempts[attempt_id]
        self.require(attempt.state == "RESERVED")
        self.require(self.reservations.get(attempt.ticket) == attempt_id)
        ticket = self.tickets[attempt.ticket]
        self.require(attempt.scope == ticket.scope and attempt.fences == ticket.fences)
        # Scope, authority, cancellation, predecessor progress and final failure
        # are checked in the same decision that installs the complete cohort.
        commit = super().accept(attempt.ticket, failure=failure)
        attempt.state = "ACCEPTED"
        del self.reservations[attempt.ticket]
        for scene in attempt.scope.values():
            self.scenes[scene].seals.discard(("attempt", attempt_id))
        return commit


class ReservationTests(unittest.TestCase):
    def test_failed_allocation_and_attempt_leave_ticket_retryable(self):
        model = ReservationModel(outputs=2)
        old = dict(model.current)
        ticket = model.prepare([0, 1])
        with self.assertRaises(Rejected):
            model.reserve(ticket, allocation_failure=True)
        self.assertEqual(model.current, old)
        self.assertFalse(model.attempts)
        self.assertFalse(model.reservations)
        attempt = model.reserve(ticket)
        with self.assertRaises(Rejected):
            model.accept_attempt(attempt, failure=True)
        self.assertEqual(model.current, old)
        model.abort(attempt)
        self.assertEqual(model.tickets[ticket].state, "READY")
        for scene in old.values():
            self.assertEqual(model.scenes[scene].seals, {("ticket", ticket)})
        retry = model.reserve(ticket)
        commit = model.accept_attempt(retry)
        model.complete_commit(commit)
        self.assertTrue(all(model.scenes[s].retired for s in old.values()))

    def test_close_rejects_acceptance_but_attempt_retains_admission(self):
        model = ReservationModel(outputs=2)
        ticket = model.prepare([0, 1])
        attempt = model.reserve(ticket)
        old = dict(model.current)
        model.close(ticket)
        with self.assertRaises(Rejected):
            model.accept_attempt(attempt)
        model.queue()
        for output, scene in old.items():
            self.assertEqual(model.scenes[scene].seals, {("attempt", attempt)})
            with self.assertRaises(Rejected):
                model.claim_source(output)
        model.abort(attempt)
        claim = model.claim_source(0)
        model.release(claim, [])

    def test_reserved_ticket_cannot_be_consumed_twice(self):
        model = ReservationModel()
        ticket = model.prepare([0])
        attempt = model.reserve(ticket)
        with self.assertRaises(Rejected):
            model.reserve(ticket)
        with self.assertRaises(Rejected):
            model.accept(ticket)
        model.accept(ticket, test_only=True)
        commit = model.accept_attempt(attempt)
        with self.assertRaises(Rejected):
            model.accept_attempt(attempt)
        with self.assertRaises(Rejected):
            model.abort(attempt)
        model.close(ticket)
        model.complete_commit(commit)

    def test_competing_commit_invalidates_reserved_scope_without_waiting(self):
        model = ReservationModel(outputs=2)
        ticket = model.prepare([0, 1])
        attempt = model.reserve(ticket)
        old = dict(model.current)
        competing = model.prepare([1])
        commit = model.accept(competing)
        self.assertEqual(model.tickets[ticket].state, "STALE")
        with self.assertRaises(Rejected):
            model.accept_attempt(attempt)
        self.assertEqual(model.current[0], old[0])
        self.assertNotEqual(model.current[1], old[1])
        model.abort(attempt)
        model.complete_commit(commit)
        self.assertFalse(model.scenes[old[0]].seals)

    def test_authority_and_worker_loss_reject_reserved_attempts(self):
        for action in ("change_authority", "worker_lost"):
            with self.subTest(action=action):
                model = ReservationModel()
                ticket = model.prepare([0])
                attempt = model.reserve(ticket)
                old = dict(model.current)
                getattr(model, action)()
                with self.assertRaises(Rejected):
                    model.accept_attempt(attempt)
                self.assertEqual(model.current, old)
                model.abort(attempt)
                self.assertFalse(model.scenes[old[0]].seals)

    def test_reservation_does_not_block_an_independent_output(self):
        model = ReservationModel(outputs=2)
        ticket = model.prepare([0])
        attempt = model.reserve(ticket)
        independent = model.accept(model.prepare([1]))
        model.complete_commit(independent)
        self.assertEqual(model.tickets[ticket].state, "READY")
        commit = model.accept_attempt(attempt)
        model.complete_commit(commit)

    def test_acceptance_keeps_native_readers_after_all_ticket_owners_close(self):
        model = ReservationModel(outputs=2)
        for output in (0, 1):
            model.queue()
            claim = model.claim_source(output)
            fence = model.submit_source(claim)
            model.release(claim, [fence])
        old = dict(model.current)
        ticket = model.prepare([0, 1])
        attempt = model.reserve(ticket)
        fences = model.attempts[attempt].fences
        commit = model.accept_attempt(attempt)
        model.close(ticket)
        for scene in old.values():
            self.assertEqual(model.scenes[scene].seals, {("commit", commit)})
        for fence in fences:
            with self.assertRaises(Rejected):
                model.complete_commit(commit)
            model.signal(fence)
        model.complete_commit(commit)

    def test_close_failure_abort_and_acceptance_event_orders(self):
        # Enumerate scheduling boundaries, not instruction-level kernel races.
        accepted = aborted = reserved = 0
        for events in permutations(("reserve", "close", "fail", "abort", "accept")):
            with self.subTest(events=events):
                model = ReservationModel(outputs=2)
                old = dict(model.current)
                ticket = model.prepare([0, 1])
                attempt = commit = None
                for event in events:
                    try:
                        if event == "reserve":
                            attempt = model.reserve(ticket)
                            reserved += 1
                        elif event == "close":
                            model.close(ticket)
                        elif attempt is None:
                            continue
                        elif event == "abort":
                            model.abort(attempt)
                            aborted += 1
                        else:
                            result = model.accept_attempt(attempt, failure=event == "fail")
                            if event == "accept":
                                commit = result
                                accepted += 1
                    except Rejected:
                        pass
                    if commit is None:
                        self.assertEqual(model.current, old)
                    else:
                        self.assertTrue(all(model.current[o] != s for o, s in old.items()))
                    for scene in old.values():
                        owners = set()
                        if model.tickets[ticket].state in ("SEALING", "READY"):
                            owners.add(("ticket", ticket))
                        if attempt is not None and model.attempts[attempt].state == "RESERVED":
                            owners.add(("attempt", attempt))
                        if commit is not None:
                            owners.add(("commit", commit))
                        self.assertEqual(model.scenes[scene].seals, owners)
                if attempt is not None and model.attempts[attempt].state == "RESERVED":
                    model.abort(attempt)
                if commit is not None:
                    model.complete_commit(commit)
                self.assertFalse(model.reservations)
                self.assertTrue(all(not model.scenes[s].seals for s in old.values()))
        self.assertGreater(reserved, 0)
        self.assertGreater(aborted, 0)
        self.assertGreater(accepted, 0)


if __name__ == "__main__":
    unittest.main()
