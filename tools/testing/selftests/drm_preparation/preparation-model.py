#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0
"""Sequential reference model, not a kernel implementation or proposed UAPI.

Each method is one serialized protocol decision. Native submission and worker
release are deliberately separate decisions. No operation simulates kernel
revocation of GPU imports or treats worker death as native completion.
"""

from copy import deepcopy
from dataclasses import dataclass, field
from itertools import permutations
import unittest


class Rejected(Exception):
    pass


@dataclass
class Scene:
    generation: int
    claims: set = field(default_factory=set)
    fences: set = field(default_factory=set)
    seals: set = field(default_factory=set)
    retired: bool = False


@dataclass
class Claim:
    scene: int
    staging: int
    submitted: set = field(default_factory=set)
    released: bool = False
    cancel_requested: bool = False


@dataclass
class Ticket:
    scope: dict
    epoch: int
    state: str = "SEALING"
    fences: frozenset = frozenset()


class Model:
    def __init__(self, outputs=1, staging_depth=2):
        self.serial = 0
        self.epoch = 1
        self.lost = False
        self.scenes = {}
        self.current = {}
        self.claims = {}
        self.tickets = {}
        self.commits = {}
        self.native = {}
        self.staging = {i: None for i in range(staging_depth)}
        self.demand = 0
        for output in range(outputs):
            self.current[output] = self.new_scene()

    def identity(self):
        self.serial += 1
        return self.serial

    def new_scene(self):
        key = self.identity()
        self.scenes[key] = Scene(key)
        return key

    def require(self, condition):
        if not condition:
            raise Rejected()

    def queue(self):
        # Queuing demand grants no source access and retains no source scene.
        self.demand += 1

    def claim_source(self, output):
        self.require(not self.lost and self.demand > 0)
        scene_id = self.current[output]
        scene = self.scenes[scene_id]
        free = next((slot for slot, owner in self.staging.items()
                     if owner is None), None)
        self.require(not scene.seals and free is not None)
        key = self.identity()
        self.claims[key] = Claim(scene_id, free)
        self.staging[free] = key
        scene.claims.add(key)
        self.demand -= 1
        return key

    def submit_source(self, claim_id):
        claim = self.claims[claim_id]
        self.require(not self.lost and not claim.released)
        # A-to-E only: the available private slot has no downstream dependency.
        fence = self.identity()
        self.native[fence] = None
        claim.submitted.add(fence)
        return fence

    def release(self, claim_id, fences):
        claim = self.claims[claim_id]
        self.require(not self.lost)
        # Validate and enroll the same owned set, including one-shot iterators.
        fences = frozenset(fences)
        # Trusted-worker oracle: coverage is asserted, not discovered from GPU
        # mappings. Repeating the same release is safe after a lost reply.
        self.require(fences == claim.submitted)
        if claim.released:
            return
        claim.released = True
        scene = self.scenes[claim.scene]
        scene.fences.update(fences)
        scene.claims.remove(claim_id)
        if not fences:
            self.staging[claim.staging] = None

    def request_cancel(self, claim_id):
        # A request cannot prove a live worker has stopped accessing the source.
        self.claims[claim_id].cancel_requested = True

    def signal(self, fence, status=0):
        self.require(fence in self.native and self.native[fence] is None)
        self.native[fence] = status

    def prepare(self, outputs):
        self.require(not self.lost and bool(outputs))
        scope = {output: self.current[output] for output in outputs}
        key = self.identity()
        self.tickets[key] = Ticket(scope, self.epoch)
        for scene in scope.values():
            self.scenes[scene].seals.add(("ticket", key))
        return key

    def ready(self, ticket_id):
        ticket = self.tickets[ticket_id]
        if ticket.state == "READY":
            return True
        if ticket.state != "SEALING":
            return False
        scenes = [self.scenes[key] for key in ticket.scope.values()]
        if any(scene.claims for scene in scenes):
            return False
        ticket.fences = frozenset(f for scene in scenes for f in scene.fences)
        ticket.state = "READY"
        return True

    def close(self, ticket_id):
        ticket = self.tickets[ticket_id]
        if ticket.state not in ("SEALING", "READY"):
            return
        ticket.state = "CANCELED"
        for key in ticket.scope.values():
            self.scenes[key].seals.discard(("ticket", ticket_id))

    def accept(self, ticket_id, *, test_only=False, failure=False):
        ticket = self.tickets[ticket_id]
        self.require(not self.lost and ticket.epoch == self.epoch)
        self.require(ticket.state in ("SEALING", "READY"))
        self.require(all(self.current[o] == s for o, s in ticket.scope.items()))
        # TEST_ONLY validates the supplied scope, not runtime readiness. It
        # neither waits for claims nor freezes the retirement fence set.
        if test_only:
            self.require(not failure)
            return None
        self.require(self.ready(ticket_id))
        # All pre-acceptance failures preserve the ticket seal.
        if failure:
            raise Rejected()
        commit = self.identity()
        self.commits[commit] = (tuple(ticket.scope.values()), ticket.fences)
        # Installation, seal transfer and ticket consumption are one decision.
        for output, old in ticket.scope.items():
            self.scenes[old].seals.add(("commit", commit))
            self.scenes[old].seals.discard(("ticket", ticket_id))
            self.current[output] = self.new_scene()
        ticket.state = "CONSUMED"
        for other_id, other in self.tickets.items():
            if other.state in ("SEALING", "READY") and any(
                    self.current[o] != s for o, s in other.scope.items()):
                self.close(other_id)
                other.state = "STALE"
        return commit

    def complete_commit(self, commit_id):
        scenes, fences = self.commits[commit_id]
        self.require(all(self.native[f] is not None for f in fences))
        for key in scenes:
            scene = self.scenes[key]
            self.require(not scene.claims)
            scene.retired = True
            scene.seals.discard(("commit", commit_id))
        del self.commits[commit_id]

    def recycle_staging(self, claim_id, downstream_done):
        claim = self.claims[claim_id]
        self.require(claim.released and downstream_done)
        self.require(all(self.native[f] is not None for f in claim.submitted))
        self.require(self.staging[claim.staging] == claim_id)
        self.staging[claim.staging] = None

    def worker_lost(self):
        self.lost = True
        self.epoch += 1
        for ticket_id, ticket in self.tickets.items():
            if ticket.state in ("SEALING", "READY"):
                self.close(ticket_id)
                ticket.state = "LOST"
        # Known fences and unresolved claims remain. Death proves neither
        # completion nor complete knowledge of unreported native work.


class PreparationTests(unittest.TestCase):
    def claimed(self, model, output=0):
        model.queue()
        return model.claim_source(output)

    def test_ready_is_not_native_completion(self):
        model = Model()
        claim = self.claimed(model)
        fence = model.submit_source(claim)
        ticket = model.prepare([0])
        self.assertFalse(model.ready(ticket))
        model.release(claim, [fence])
        self.assertTrue(model.ready(ticket))
        commit = model.accept(ticket)
        with self.assertRaises(Rejected):
            model.complete_commit(commit)
        model.signal(fence)
        model.complete_commit(commit)

    def test_release_enrolls_the_validated_fence_iterator(self):
        model = Model()
        claim = self.claimed(model)
        fence = model.submit_source(claim)
        model.release(claim, (entry for entry in [fence]))
        ticket = model.prepare([0])
        self.assertTrue(model.ready(ticket))
        self.assertEqual(model.tickets[ticket].fences, frozenset([fence]))
        commit = model.accept(ticket)
        with self.assertRaises(Rejected):
            model.complete_commit(commit)
        model.signal(fence)
        model.complete_commit(commit)

    def test_cancellation_needs_worker_release(self):
        model = Model()
        claim = self.claimed(model)
        ticket = model.prepare([0])
        model.request_cancel(claim)
        self.assertFalse(model.ready(ticket))
        # Submission may race the cancellation request for an existing claim.
        fence = model.submit_source(claim)
        model.release(claim, [fence])
        self.assertTrue(model.ready(ticket))

    def test_no_access_release_is_idempotent(self):
        model = Model()
        claim = self.claimed(model)
        ticket = model.prepare([0])
        model.release(claim, [])
        model.release(claim, [])
        self.assertTrue(model.ready(ticket))
        with self.assertRaises(Rejected):
            model.submit_source(claim)

    def test_close_after_acceptance_preserves_retirement_guard(self):
        model = Model()
        old = model.current[0]
        claim = self.claimed(model)
        fence = model.submit_source(claim)
        model.release(claim, [fence])
        ticket = model.prepare([0])
        commit = model.accept(ticket)
        model.close(ticket)
        self.assertEqual(model.scenes[old].seals, {("commit", commit)})
        with self.assertRaises(Rejected):
            model.complete_commit(commit)

    def test_rejected_attempt_and_test_only_preserve_ticket(self):
        model = Model()
        old = model.current[0]
        ticket = model.prepare([0])
        model.accept(ticket, test_only=True)
        with self.assertRaises(Rejected):
            model.accept(ticket, failure=True)
        self.assertEqual(model.current[0], old)
        self.assertEqual(model.scenes[old].seals, {("ticket", ticket)})
        model.accept(ticket)
        with self.assertRaises(Rejected):
            model.accept(ticket)

    def test_overlapping_cohorts_do_not_wait_for_competing_commits(self):
        model = Model(outputs=3)
        first = model.prepare([0, 1])
        second = model.prepare([1, 2])
        self.assertTrue(model.ready(first))
        self.assertTrue(model.ready(second))
        model.accept(second)
        self.assertEqual(model.tickets[first].state, "STALE")
        self.assertFalse(model.scenes[model.current[0]].seals)

    def test_test_only_does_not_resolve_preparation(self):
        model = Model()
        claim = self.claimed(model)
        fence = model.submit_source(claim)
        ticket = model.prepare([0])
        # Check both unresolved claims and a releasable but unpolled ticket,
        # then an already-ready ticket whose reader is still running.
        for phase in ("claimed", "released", "ready"):
            with self.subTest(phase=phase):
                if phase == "released":
                    model.release(claim, [fence])
                elif phase == "ready":
                    self.assertTrue(model.ready(ticket))
                before = deepcopy(model.__dict__)
                self.assertIsNone(model.accept(ticket, test_only=True))
                self.assertEqual(model.__dict__, before)
                with self.assertRaises(Rejected):
                    model.accept(ticket, test_only=True, failure=True)
                self.assertEqual(model.__dict__, before)
        commit = model.accept(ticket)
        with self.assertRaises(Rejected):
            model.complete_commit(commit)
        model.signal(fence)
        model.complete_commit(commit)

    def test_test_only_rejects_invalid_ticket_without_mutation(self):
        for state in ("CANCELED", "CONSUMED", "STALE", "LOST"):
            with self.subTest(state=state):
                model = Model()
                ticket = model.prepare([0])
                if state == "CANCELED":
                    model.close(ticket)
                elif state == "CONSUMED":
                    model.accept(ticket)
                elif state == "STALE":
                    model.accept(model.prepare([0]))
                else:
                    model.worker_lost()
                self.assertEqual(model.tickets[ticket].state, state)
                before = deepcopy(model.__dict__)
                with self.assertRaises(Rejected):
                    model.accept(ticket, test_only=True)
                self.assertEqual(model.__dict__, before)

    def test_last_close_reopens_without_forgetting_readers(self):
        model = Model()
        claim = self.claimed(model)
        fence = model.submit_source(claim)
        model.release(claim, [fence])
        first = model.prepare([0])
        second = model.prepare([0])
        model.close(first)
        model.queue()
        with self.assertRaises(Rejected):
            model.claim_source(0)
        model.close(second)
        model.claim_source(0)
        self.assertIn(fence, model.scenes[model.current[0]].fences)

    def test_destination_stall_does_not_retain_sources(self):
        model = Model(staging_depth=1)
        old = model.current[0]
        claim = self.claimed(model)
        fence = model.submit_source(claim)
        model.release(claim, [fence])
        commit = model.accept(model.prepare([0]))
        model.signal(fence)
        model.complete_commit(commit)
        self.assertTrue(model.scenes[old].retired)
        with self.assertRaises(Rejected):
            model.recycle_staging(claim, downstream_done=False)
        model.queue()
        with self.assertRaises(Rejected):
            model.claim_source(0)
        # Demand remains source-unbound while independently retained E is full.
        for _ in range(10):
            commit = model.accept(model.prepare([0]))
            model.complete_commit(commit)
        self.assertEqual(model.demand, 1)
        self.assertFalse(model.scenes[model.current[0]].claims)

    def test_native_failure_ends_access(self):
        model = Model()
        claim = self.claimed(model)
        fence = model.submit_source(claim)
        model.release(claim, [fence])
        commit = model.accept(model.prepare([0]))
        model.signal(fence, status=-5)
        model.complete_commit(commit)
        self.assertEqual(model.native[fence], -5)  # Not successful pixels.

    def test_worker_loss_is_not_ready_or_native_completion(self):
        model = Model()
        claim = self.claimed(model)
        fence = model.submit_source(claim)
        ticket = model.prepare([0])
        model.worker_lost()
        self.assertFalse(model.ready(ticket))
        self.assertIsNone(model.native[fence])
        self.assertIn(claim, model.scenes[model.current[0]].claims)
        with self.assertRaises(Rejected):
            model.accept(ticket)

    def test_nominal_schedule_produces_frames_at_independent_depths(self):
        for depth in (1, 2, 4, 8):
            model = Model(staging_depth=depth)
            frames = 0
            for _ in range(60):
                claim = self.claimed(model)
                ticket = model.prepare([0])
                fence = model.submit_source(claim)
                model.release(claim, [fence])
                commit = model.accept(ticket)
                model.signal(fence)
                model.complete_commit(commit)
                model.recycle_staging(claim, downstream_done=True)
                frames += 1
            self.assertEqual(frames, 60)
            self.assertTrue(all(owner is None for owner in model.staging.values()))

    def test_serialized_close_accept_release_interleavings(self):
        # Enumerate a bounded event set, not every possible execution. An event
        # whose input object does not yet exist is an unavailable action.
        events = ("submit", "release", "prepare", "ready", "accept", "close", "signal", "complete")
        accepted = retired = ready_observed = 0
        for schedule in permutations(events):
            model = Model()
            old = model.current[0]
            claim = self.claimed(model)
            fence = ticket = commit = None
            for event in schedule:
                try:
                    if event == "submit":
                        fence = model.submit_source(claim)
                    elif event == "release":
                        model.release(claim, model.claims[claim].submitted)
                    elif event == "prepare":
                        ticket = model.prepare([0])
                    elif event == "ready" and ticket is not None:
                        model.ready(ticket)
                    elif event == "accept" and ticket is not None:
                        commit = model.accept(ticket)
                        accepted += 1
                    elif event == "close" and ticket is not None:
                        model.close(ticket)
                    elif event == "signal" and fence is not None:
                        model.signal(fence)
                    elif event == "complete" and commit is not None:
                        model.complete_commit(commit)
                        retired += 1
                except Rejected:
                    pass  # An ordinary precondition failure changes no ownership.
                scene = model.scenes[old]
                if commit is not None and not scene.retired:
                    self.assertIn(("commit", commit), scene.seals, schedule)
                if scene.retired:
                    self.assertFalse(scene.claims, schedule)
                    self.assertTrue(all(model.native[f] is not None
                                        for f in scene.fences), schedule)
                if ticket is not None and model.tickets[ticket].state == "READY":
                    ready_observed += 1
                    self.assertFalse(scene.claims, schedule)
                    self.assertIn(("ticket", ticket), scene.seals, schedule)
        # A suite consisting exclusively of rejected operations is not useful.
        self.assertGreater(accepted, 0)
        self.assertGreater(retired, 0)
        self.assertGreater(ready_observed, 0)


if __name__ == "__main__":
    unittest.main()
