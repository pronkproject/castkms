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
    scanout: object = None
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
    authority: int
    state: str = "SEALING"
    fences: frozenset = frozenset()


@dataclass(frozen=True)
class RequestedScanout:
    output: int
    framebuffer: object
    producer: int
    x: int


@dataclass(frozen=True)
class OwnedRequest:
    owner: object
    epoch: int
    ticket: int
    changes: tuple


class Model:
    def __init__(self, outputs=1, staging_depth=2):
        self.serial = 0
        self.epoch = 1
        self.authority = 1
        self.lost = False
        self.scenes = {}
        self.current = {}
        self.claims = {}
        self.tickets = {}
        self.commits = {}
        self.pending = {}
        self.native = {}
        self.native_waits = {}
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
        self.require(all(self.native[dependency] is not None
                         for dependency in self.native_waits.get(fence, ())))
        self.native[fence] = status

    def submit_native(self, dependencies=()):
        # Only previously submitted work may be a dependency. A new fence
        # cannot wait on itself or a future submission in this fake provider.
        dependencies = frozenset(dependencies)
        self.require(all(fence in self.native for fence in dependencies))
        fence = self.identity()
        self.native[fence] = None
        self.native_waits[fence] = dependencies
        return fence

    def prepare(self, outputs):
        self.require(not self.lost and bool(outputs))
        scope = {output: self.current[output] for output in outputs}
        key = self.identity()
        self.tickets[key] = Ticket(scope, self.epoch, self.authority)
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

    def capture_request(self, rows, framebuffers, fence_fds, ticket_fds, ticket_fd):
        # Fake caller namespaces: framebuffer IDs and fd numbers are reusable.
        # The request retains their referents, never their lookup keys. Rows
        # contain output, framebuffer ID, producer fd and a scalar property.
        self.require(0 < len(rows) <= len(self.current))
        changes = []
        outputs = set()
        for output, framebuffer_id, producer_fd, x in rows:
            self.require(output in self.current and output not in outputs)
            self.require(isinstance(x, int))
            self.require(framebuffer_id in framebuffers and producer_fd in fence_fds)
            producer = fence_fds[producer_fd]
            self.require(producer in self.native)
            changes.append(RequestedScanout(output, framebuffers[framebuffer_id],
                                            producer, x))
            outputs.add(output)
        self.require(ticket_fd in ticket_fds)
        ticket = ticket_fds[ticket_fd]
        self.require(ticket in self.tickets)
        # The immutable tuple owns framebuffer references. Native fence and
        # ticket identities name objects retained by the model's oracle maps.
        return OwnedRequest(self, self.epoch, ticket, tuple(changes))

    def rebuild_request(self, request):
        self.require(request.owner is self and request.epoch == self.epoch)
        ticket = self.tickets[request.ticket]
        self.require({change.output for change in request.changes} == set(ticket.scope))
        self.accept(request.ticket, test_only=True)
        # Reconstruct from current state, not a checked state kept across a
        # host wait. Unchanged outputs inherit their newly observed values.
        state = {output: self.scenes[scene].scanout
                 for output, scene in self.current.items()}
        state.update((change.output, change) for change in request.changes)
        return state

    def accept_request(self, request, *, test_only=False, failure=False):
        # Rebuild and acceptance are one serialized decision. No caller may
        # hand in an earlier checked state as a substitute for reconstruction.
        state = self.rebuild_request(request)
        commit = self.accept(request.ticket, test_only=test_only, failure=failure)
        if not test_only:
            for change in request.changes:
                self.scenes[self.current[change.output]].scanout = state[change.output]
        return commit

    def accept(self, ticket_id, *, test_only=False, failure=False):
        ticket = self.tickets[ticket_id]
        self.require(not self.lost and ticket.epoch == self.epoch)
        self.require(ticket.authority == self.authority)
        self.require(ticket.state in ("SEALING", "READY"))
        self.require(all(self.current[o] == s for o, s in ticket.scope.items()))
        # TEST_ONLY validates the supplied scope, not runtime readiness. It
        # neither waits for claims nor freezes the retirement fence set.
        if test_only:
            self.require(not failure)
            return None
        # Conservative test-provider policy: an output accepts another update
        # after its preceding commit completes. Readiness remains independent.
        self.require(all(output not in self.pending for output in ticket.scope))
        self.require(self.ready(ticket_id))
        # All pre-acceptance failures preserve the ticket seal.
        if failure:
            raise Rejected()
        commit = self.identity()
        self.commits[commit] = (dict(ticket.scope), ticket.fences)
        # Installation, seal transfer and ticket consumption are one decision.
        for output, old in ticket.scope.items():
            self.scenes[old].seals.add(("commit", commit))
            self.scenes[old].seals.discard(("ticket", ticket_id))
            self.current[output] = self.new_scene()
            self.pending[output] = commit
        ticket.state = "CONSUMED"
        for other_id, other in self.tickets.items():
            if other.state in ("SEALING", "READY") and any(
                    self.current[o] != s for o, s in other.scope.items()):
                self.close(other_id)
                other.state = "STALE"
        return commit

    def complete_commit(self, commit_id):
        scope, fences = self.commits[commit_id]
        self.require(all(self.native[f] is not None for f in fences))
        # The fake provider has one pending commit per output, so current
        # still names each accepted replacement. Normal display completion
        # must also wait for that replacement's producer, not for a daemon.
        scanouts = [self.scenes[self.current[output]].scanout for output in scope]
        self.require(all(scanout is None or self.native[scanout.producer] is not None
                         for scanout in scanouts))
        for output, key in scope.items():
            scene = self.scenes[key]
            self.require(not scene.claims)
            scene.retired = True
            scene.seals.discard(("commit", commit_id))
            del self.pending[output]
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

    def change_authority(self):
        # A replacement modesetting authority may issue fresh tickets. Old
        # references must not extend the previous authority's right to commit.
        self.authority += 1
        for ticket_id, ticket in self.tickets.items():
            if ticket.state in ("SEALING", "READY"):
                self.close(ticket_id)
                ticket.state = "REVOKED"


class PreparationTests(unittest.TestCase):
    def claimed(self, model, output=0):
        model.queue()
        return model.claim_source(output)

    def request(self, model, outputs=(0,), framebuffer="frame", x=12,
                producer_status=0):
        # An independent, already submitted producer for the new framebuffer.
        producer = model.submit_native()
        if producer_status is not None:
            model.signal(producer, producer_status)
        ticket = model.prepare(outputs)
        rows = [[output, 7, 8, x] for output in outputs]
        return model.capture_request(rows, {7: framebuffer}, {8: producer},
                                     {9: ticket}, 9)

    def test_native_dependencies_finish_before_dependent_work(self):
        model = Model()
        producer = model.submit_native()
        other = model.submit_native()
        copy = model.submit_native(iter([producer, other, producer]))
        self.assertEqual(model.native_waits[copy], frozenset([producer, other]))
        before = deepcopy(model.__dict__)
        with self.assertRaises(Rejected):
            model.signal(copy)
        self.assertEqual(model.__dict__, before)
        model.signal(producer, status=-5)
        with self.assertRaises(Rejected):
            model.signal(copy)
        model.signal(other)
        # A dependency ending access does not promise its pixels are valid.
        # The provider deliberately permits a successful dependent fence.
        model.signal(copy)
        self.assertEqual(model.native[copy], 0)
        self.assertEqual(model.native[producer], -5)
        with self.assertRaises(Rejected):
            model.signal(producer)

    def test_native_submission_rejects_unknown_dependencies_atomically(self):
        model = Model()
        producer = model.submit_native()
        before = deepcopy(model.__dict__)
        with self.assertRaises(Rejected):
            model.submit_native([producer, -1])
        self.assertEqual(model.__dict__, before)

    def test_display_completion_waits_for_replacement_producer(self):
        for producer_first in (False, True):
            for status in (0, -5):
                with self.subTest(producer_first=producer_first, status=status):
                    model = Model()
                    old = model.current[0]
                    request = self.request(model, producer_status=None)
                    producer = request.changes[0].producer
                    if producer_first:
                        model.signal(producer, status)
                    self.assertTrue(model.ready(request.ticket))
                    commit = model.accept_request(request)
                    if not producer_first:
                        before = deepcopy(model.__dict__)
                        with self.assertRaises(Rejected):
                            model.complete_commit(commit)
                        self.assertEqual(model.__dict__, before)
                        self.assertFalse(model.scenes[old].retired)
                        model.signal(producer, status)
                    model.complete_commit(commit)
                    self.assertTrue(model.scenes[old].retired)
                    self.assertEqual(model.native[producer], status)

    def test_waiting_request_is_rebuilt_from_current_display_state(self):
        model = Model(outputs=2)
        predecessor = model.accept(model.prepare([0]))
        request = self.request(model)
        checked_before_wait = model.rebuild_request(request)
        self.assertIsNone(checked_before_wait[1])
        self.assertTrue(model.ready(request.ticket))
        with self.assertRaises(Rejected):
            model.accept_request(request)
        independent = self.request(model, outputs=(1,), framebuffer="other")
        model.complete_commit(model.accept_request(independent))
        model.complete_commit(predecessor)
        rebuilt = model.rebuild_request(request)
        self.assertEqual(rebuilt[1], independent.changes[0])
        self.assertIsNone(checked_before_wait[1])
        model.complete_commit(model.accept_request(request))
        self.assertEqual(model.scenes[model.current[0]].scanout, request.changes[0])
        self.assertEqual(model.scenes[model.current[1]].scanout, independent.changes[0])

    def test_request_retry_preserves_ticket_until_acceptance(self):
        model = Model()
        claim = self.claimed(model)
        request = self.request(model)
        before = deepcopy(model.__dict__)
        model.accept_request(request, test_only=True)
        self.assertEqual(model.__dict__, before)
        with self.assertRaises(Rejected):
            model.accept_request(request)
        self.assertEqual(model.__dict__, before)
        model.release(claim, [])
        self.assertTrue(model.ready(request.ticket))
        before = deepcopy(model.__dict__)
        with self.assertRaises(Rejected):
            model.accept_request(request, failure=True)
        self.assertEqual(model.__dict__, before)
        commit = model.accept_request(request)
        before = deepcopy(model.__dict__)
        # Losing result delivery after acceptance does not authorize replay.
        with self.assertRaises(Rejected):
            model.accept_request(request)
        self.assertEqual(model.__dict__, before)
        model.complete_commit(commit)

    def test_rebuild_rejects_obsolete_ticket_after_competing_update(self):
        model = Model()
        request = self.request(model)
        model.rebuild_request(request)
        winner = model.accept(model.prepare([0]))
        model.complete_commit(winner)
        before = deepcopy(model.__dict__)
        with self.assertRaises(Rejected):
            model.accept_request(request)
        self.assertEqual(model.__dict__, before)
        replacement = self.request(model)
        model.complete_commit(model.accept_request(replacement))

    def test_rebuild_rejects_wrong_scope_or_device_without_consumption(self):
        model = Model(outputs=2)
        request = self.request(model)
        # A caller's wider request must not retire an unprepared output.
        wider = model.capture_request([[0, 7, 8, 12], [1, 7, 8, 12]],
                                      {7: "frame"}, {8: request.changes[0].producer},
                                      {9: request.ticket}, 9)
        for candidate, target in ((wider, model), (request, Model(outputs=2))):
            before = deepcopy(target.__dict__)
            with self.assertRaises(Rejected):
                target.accept_request(candidate)
            self.assertEqual(target.__dict__, before)
        self.assertEqual(model.tickets[request.ticket].state, "SEALING")

    def test_worker_loss_invalidates_owned_request(self):
        model = Model()
        request = self.request(model)
        model.rebuild_request(request)
        model.worker_lost()
        before = deepcopy(model.__dict__)
        with self.assertRaises(Rejected):
            model.accept_request(request)
        self.assertEqual(model.__dict__, before)

    def test_authority_change_rejects_waiting_request_without_revival(self):
        for ready in (False, True):
            with self.subTest(ready=ready):
                model = Model()
                request = self.request(model)
                old = model.current[0]
                if ready:
                    self.assertTrue(model.ready(request.ticket))
                model.rebuild_request(request)
                model.change_authority()
                self.assertEqual(model.tickets[request.ticket].state, "REVOKED")
                self.assertFalse(model.scenes[old].seals)
                before = deepcopy(model.__dict__)
                for test_only in (False, True):
                    with self.assertRaises(Rejected):
                        model.accept_request(request, test_only=test_only)
                    self.assertEqual(model.__dict__, before)
                # A new authority can prepare the unchanged picture, but may
                # not substitute its ticket through the old request's fd.
                fresh = self.request(model)
                model.complete_commit(model.accept_request(fresh))
                with self.assertRaises(Rejected):
                    model.accept_request(request)

    def test_authority_change_preserves_accepted_reader_dependencies(self):
        model = Model()
        claim = self.claimed(model)
        fence = model.submit_source(claim)
        model.release(claim, [fence])
        request = self.request(model)
        commit = model.accept_request(request)
        old = model.claims[claim].scene
        model.change_authority()
        model.close(request.ticket)
        self.assertEqual(model.scenes[old].seals, {("commit", commit)})
        with self.assertRaises(Rejected):
            model.complete_commit(commit)
        model.signal(fence)
        model.complete_commit(commit)

    def test_request_capture_owns_values_instead_of_lookup_keys(self):
        model = Model()
        claim = self.claimed(model)
        fence = model.submit_source(claim)
        ticket = model.prepare([0])
        framebuffer = object()
        rows = [[0, 7, 8, 12]]
        framebuffers, fence_fds, ticket_fds = {7: framebuffer}, {8: fence}, {9: ticket}
        before = deepcopy(model.__dict__)
        request = model.capture_request(rows, framebuffers, fence_fds, ticket_fds, 9)
        self.assertEqual(model.__dict__, before)
        rows[0][:] = [99, 99, 99, 99]
        rows.clear()
        framebuffers.clear()
        fence_fds.clear()
        ticket_fds.clear()
        framebuffers[7], fence_fds[8], ticket_fds[9] = object(), -1, -1
        self.assertIs(request.owner, model)
        self.assertEqual(request.ticket, ticket)
        change, = request.changes
        self.assertEqual((change.output, change.producer, change.x), (0, fence, 12))
        self.assertIs(change.framebuffer, framebuffer)
        with self.assertRaises(AttributeError):
            change.x = 99
        with self.assertRaises(AttributeError):
            request.ticket = -1

    def test_request_capture_rejects_unresolved_or_unbounded_inputs(self):
        model = Model(outputs=2)
        claim = self.claimed(model)
        fence = model.submit_source(claim)
        ticket = model.prepare([0])
        valid = [0, 7, 8, 12]
        invalid = ([], [valid] * 3, [valid, valid], [[2, 7, 8, 12]],
                   [valid, [1, 99, 8, 12]], [valid, [1, 7, 99, 12]],
                   [[0, 7, 8, []]])
        for rows in invalid:
            with self.subTest(rows=rows):
                before = deepcopy(model.__dict__)
                with self.assertRaises(Rejected):
                    model.capture_request(rows, {7: object()}, {8: fence}, {9: ticket}, 9)
                self.assertEqual(model.__dict__, before)
        with self.assertRaises(Rejected):
            model.capture_request([valid], {7: object()}, {8: fence}, {}, 9)
        with self.assertRaises(Rejected):
            model.capture_request([valid], {7: object()}, {8: -1}, {9: ticket}, 9)

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

    def test_ready_ticket_waits_for_accepted_predecessor(self):
        model = Model()
        claim = self.claimed(model)
        fence = model.submit_source(claim)
        model.release(claim, [fence])
        predecessor = model.accept(model.prepare([0]))
        ticket = model.prepare([0])
        self.assertTrue(model.ready(ticket))
        before = deepcopy(model.__dict__)
        with self.assertRaises(Rejected):
            model.accept(ticket)
        self.assertEqual(model.__dict__, before)
        model.accept(ticket, test_only=True)
        self.assertEqual(model.__dict__, before)
        # Cancellation of the next update cannot release the earlier commit.
        model.close(ticket)
        self.assertEqual(model.pending, {0: predecessor})
        ticket = model.prepare([0])
        model.signal(fence)
        model.complete_commit(predecessor)
        model.complete_commit(model.accept(ticket))
        self.assertFalse(model.pending)

    def test_predecessor_resolution_does_not_release_new_claim(self):
        model = Model()
        predecessor = model.accept(model.prepare([0]))
        claim = self.claimed(model)
        ticket = model.prepare([0])
        model.complete_commit(predecessor)
        self.assertFalse(model.ready(ticket))
        with self.assertRaises(Rejected):
            model.accept(ticket)
        model.release(claim, [])
        model.complete_commit(model.accept(ticket))

    def test_busy_output_rejects_whole_cohort_without_blocking_others(self):
        model = Model(outputs=3)
        predecessor = model.accept(model.prepare([0]))
        cohort = model.prepare([0, 1])
        self.assertTrue(model.ready(cohort))
        before = deepcopy(model.__dict__)
        with self.assertRaises(Rejected):
            model.accept(cohort)
        self.assertEqual(model.__dict__, before)
        independent = model.accept(model.prepare([2]))
        model.complete_commit(independent)
        # An overlapping winner invalidates the waiting cohort; resolving its
        # predecessor must not revive the old request or replace output 0.
        winner = model.accept(model.prepare([1]))
        self.assertEqual(model.tickets[cohort].state, "STALE")
        model.complete_commit(predecessor)
        with self.assertRaises(Rejected):
            model.accept(cohort)
        model.complete_commit(winner)
        model.complete_commit(model.accept(model.prepare([0, 1])))
        self.assertFalse(model.pending)

    def test_predecessor_and_release_orders_allow_retry(self):
        for schedule in permutations(("predecessor", "release", "ready", "accept")):
            with self.subTest(schedule=schedule):
                model = Model()
                predecessor = model.accept(model.prepare([0]))
                claim = self.claimed(model)
                ticket = model.prepare([0])
                commit = None
                for event in schedule:
                    try:
                        if event == "predecessor":
                            model.complete_commit(predecessor)
                        elif event == "release":
                            model.release(claim, [])
                        elif event == "ready":
                            model.ready(ticket)
                        else:
                            commit = model.accept(ticket)
                    except Rejected:
                        self.assertEqual(event, "accept")
                    if commit is not None:
                        self.assertNotIn(predecessor, model.commits)
                        self.assertTrue(model.claims[claim].released)
                    else:
                        scene = model.scenes[model.current[0]]
                        self.assertIn(("ticket", ticket), scene.seals)
                if commit is None:
                    commit = model.accept(ticket)
                model.complete_commit(commit)
                self.assertFalse(model.pending)

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
