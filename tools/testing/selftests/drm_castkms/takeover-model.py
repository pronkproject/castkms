#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Model private renderer preparation and serialized execution publication.

Decisions here are serialized, standing in for the kernel's required locking.
The fixture knows native submissions as an oracle for a trusted renderer's
release promise; it does not model an enforced native-driver submission gate.
Handback, image export, media negotiation and GPU interoperability are outside
this model. A successful private probe is not a captured display frame.
Probe capacity bounds pending native cleanup, not allocation bytes or retained
exports; the kernel's independent snapshot-budget tests cover that other limit.
"""

from dataclasses import dataclass, field
import unittest


class Rejected(Exception):
    pass


@dataclass(eq=False)
class Fence:
    status: int | None = None


@dataclass(eq=False)
class Storage:
    creator: object
    kind: str


@dataclass(eq=False)
class Probe:
    resources: object
    storage: Storage
    submitted: list = field(default_factory=list)
    released: bool = False
    lost: bool = False


@dataclass(eq=False)
class Candidate:
    owner: object
    authority: int
    configuration: object
    capabilities: int
    resources: object = None
    probe: Probe | None = None
    state: str = "preparing"
    receipt: int | None = None


@dataclass(frozen=True)
class Update:
    owner: object
    gpu_only: bool
    configuration: object


class Execution:
    def __init__(self, probe_capacity=2):
        self.require(probe_capacity > 0)
        self.probe_capacity = probe_capacity
        self.authority = 1
        self.configuration = object()
        self.capabilities = 1
        self.enabled = True
        self.content = 0
        self.generation = 1
        self.mode = "host"
        self.candidate = None
        self.active = None
        self.candidates = set()
        self.host_reads = set()
        self.completed_host_reads = 0
        self.retained_probes = []
        self.live_claims = 0
        self.pending = None

    @staticmethod
    def require(condition):
        if not condition:
            raise Rejected()

    def known(self, candidate):
        self.require(candidate.owner is self and candidate in self.candidates)

    def current(self, candidate):
        self.known(candidate)
        self.require(self.enabled and candidate.authority == self.authority)
        self.require(candidate.configuration is self.configuration)
        self.require(candidate.capabilities == self.capabilities)
        self.require(candidate is self.candidate and candidate.state == "preparing")

    def begin(self):
        self.require(self.enabled and self.mode == "host" and self.candidate is None)
        candidate = Candidate(self, self.authority, self.configuration, self.capabilities)
        self.candidates.add(candidate)
        self.candidate = candidate
        return candidate

    def register(self, candidate):
        self.current(candidate)
        candidate.resources = object()
        return candidate.resources

    def private_storage(self, candidate):
        self.current(candidate)
        return Storage(candidate, "private")

    def start_probe(self, candidate, storage):
        self.current(candidate)
        self.require(candidate.resources is not None and candidate.probe is None)
        self.require(storage.creator is candidate and storage.kind == "private")
        occupied = len(self.retained_probes) + (self.active is not None)
        self.require(occupied < self.probe_capacity)
        candidate.probe = Probe(candidate.resources, storage)
        return candidate.probe

    def submit_probe(self, candidate):
        self.current(candidate)
        probe = candidate.probe
        self.require(probe is not None and not probe.released and not probe.lost)
        fence = Fence()
        probe.submitted.append(fence)
        return fence

    def release_probe(self, candidate, fences):
        self.current(candidate)
        probe = candidate.probe
        self.require(probe is not None and not probe.released and not probe.lost)
        self.require(bool(probe.submitted))
        self.require(len(fences) == len(probe.submitted))
        self.require(set(fences) == set(probe.submitted))
        probe.released = True

    def signal(self, fence, status=0):
        self.require(fence.status is None and status <= 0)
        fence.status = status

    def commit(self, candidate):
        self.known(candidate)
        self.require(self.enabled and candidate.authority == self.authority)
        if candidate is self.active and candidate.state == "active":
            return candidate.receipt
        self.current(candidate)
        self.require(self.pending is None)
        probe = candidate.probe
        self.require(probe is not None and probe.released and not probe.lost)
        self.require(probe.resources is candidate.resources)
        self.require(all(fence.status == 0 for fence in probe.submitted))
        self.generation += 1
        self.mode = "gpu"
        self.active = candidate
        self.candidate = None
        candidate.state = "active"
        candidate.receipt = self.generation
        return candidate.receipt

    def abort(self, candidate):
        self.known(candidate)
        if candidate.state != "preparing":
            return candidate.state
        candidate.state = "aborted"
        if candidate is self.candidate:
            self.candidate = None
        if candidate.probe is not None:
            candidate.probe.lost = True
            self.retained_probes.append(candidate.probe)
            candidate.probe = None
        return candidate.state

    def reap_known_completions(self):
        # Failed candidates do not invent completion for their accepted native work.
        self.retained_probes = [probe for probe in self.retained_probes
                                if any(f.status is None for f in probe.submitted)]

    def check_update(self, *, gpu_only=False, configuration=None):
        self.require(self.enabled)
        self.require(not gpu_only or self.mode == "gpu")
        return Update(self, gpu_only, configuration)

    def accept_update(self, update):
        self.require(update.owner is self and self.pending is None)
        self.require(self.enabled and (not update.gpu_only or self.mode == "gpu"))
        self.pending = update

    def install_update(self):
        self.require(self.pending is not None)
        update = self.pending
        if update.configuration is not None:
            self.configuration = update.configuration
        self.content += 1
        self.pending = None

    def update(self, *, gpu_only=False, configuration=None):
        update = self.check_update(gpu_only=gpu_only, configuration=configuration)
        self.accept_update(update)
        self.install_update()

    def claim_host(self):
        self.require(self.enabled and self.mode == "host")
        read = object()
        self.host_reads.add(read)
        return read

    def complete_host(self, read):
        self.require(read in self.host_reads)
        self.host_reads.remove(read)
        self.completed_host_reads += 1

    def claim_live(self, candidate):
        self.known(candidate)
        self.require(self.enabled and self.mode == "gpu" and candidate is self.active)
        self.require(candidate.authority == self.authority)
        self.live_claims += 1


class TakeoverTests(unittest.TestCase):
    def prepared(self, execution):
        candidate = execution.begin()
        execution.register(candidate)
        execution.start_probe(candidate, execution.private_storage(candidate))
        fence = execution.submit_probe(candidate)
        execution.release_probe(candidate, [fence])
        return candidate, fence

    def test_animation_does_not_starve_activation(self):
        for delay in (1, 2, 8, 32):
            with self.subTest(delay=delay):
                execution = Execution()
                candidate, fence = self.prepared(execution)
                for _ in range(delay):
                    execution.update()
                    execution.complete_host(execution.claim_host())
                    with self.assertRaises(Rejected):
                        execution.commit(candidate)
                execution.signal(fence)
                receipt = execution.commit(candidate)
                self.assertEqual(receipt, 2)
                self.assertEqual(execution.completed_host_reads, delay)
                self.assertEqual(execution.live_claims, 0)
                execution.claim_live(candidate)

    def test_release_and_successful_completion_are_both_required(self):
        execution = Execution()
        candidate = execution.begin()
        execution.register(candidate)
        execution.start_probe(candidate, execution.private_storage(candidate))
        first = execution.submit_probe(candidate)
        second = execution.submit_probe(candidate)
        execution.signal(first)
        execution.signal(second)
        with self.assertRaises(Rejected):
            execution.commit(candidate)
        for incomplete in ([first], [first, first], []):
            with self.assertRaises(Rejected):
                execution.release_probe(candidate, incomplete)
        execution.release_probe(candidate, [second, first])
        with self.assertRaises(Rejected):
            execution.submit_probe(candidate)
        execution.commit(candidate)

    def test_failed_native_completion_does_not_activate(self):
        execution = Execution()
        candidate, fence = self.prepared(execution)
        execution.signal(fence, -5)
        with self.assertRaises(Rejected):
            execution.commit(candidate)
        self.assertEqual(execution.mode, "host")
        execution.complete_host(execution.claim_host())

    def test_probe_cannot_use_live_or_reusable_host_storage(self):
        execution = Execution()
        candidate = execution.begin()
        execution.register(candidate)
        for kind in ("source", "host-pool", "old-output"):
            with self.assertRaises(Rejected):
                execution.start_probe(candidate, Storage(candidate, kind))
        with self.assertRaises(Rejected):
            execution.claim_live(candidate)
        execution.start_probe(candidate, execution.private_storage(candidate))

    def test_replacement_cannot_reuse_an_earlier_candidates_storage(self):
        execution = Execution()
        old = execution.begin()
        storage = execution.private_storage(old)
        execution.abort(old)
        replacement = execution.begin()
        execution.register(replacement)
        with self.assertRaises(Rejected):
            execution.start_probe(replacement, storage)
        self.assertEqual(execution.abort(old), "aborted")
        self.assertIs(execution.candidate, replacement)

    def test_candidate_loss_does_not_fail_host_readers(self):
        execution = Execution()
        host_read = execution.claim_host()
        candidate, fence = self.prepared(execution)
        execution.abort(candidate)
        execution.reap_known_completions()
        self.assertIsNone(fence.status)
        self.assertEqual(len(execution.retained_probes), 1)
        execution.complete_host(host_read)
        for _ in range(16):
            execution.update()
            execution.complete_host(execution.claim_host())
        self.assertEqual(execution.completed_host_reads, 17)
        execution.signal(fence, -5)
        execution.reap_known_completions()
        self.assertEqual(execution.retained_probes, [])

    def test_stale_binding_rejects_without_publication(self):
        for change in ("authority", "configuration", "capabilities", "disable"):
            with self.subTest(change=change):
                execution = Execution()
                candidate, fence = self.prepared(execution)
                execution.signal(fence)
                if change == "authority":
                    execution.authority += 1
                elif change == "configuration":
                    execution.update(configuration=object())
                elif change == "capabilities":
                    execution.capabilities += 1
                else:
                    execution.enabled = False
                with self.assertRaises(Rejected):
                    execution.commit(candidate)
                self.assertEqual(execution.mode, "host")
                self.assertEqual(execution.generation, 1)

    def test_changed_resource_generation_rejects_a_completed_probe(self):
        execution = Execution()
        candidate, fence = self.prepared(execution)
        execution.signal(fence)
        execution.register(candidate)
        with self.assertRaises(Rejected):
            execution.commit(candidate)
        execution.abort(candidate)
        execution.reap_known_completions()
        replacement, fence = self.prepared(execution)
        execution.signal(fence)
        execution.commit(replacement)

    def test_retained_failed_probes_have_an_independent_bound(self):
        for depth in (1, 2, 8):
            with self.subTest(depth=depth):
                execution = Execution(probe_capacity=depth)
                fences = []
                for _ in range(depth):
                    candidate, fence = self.prepared(execution)
                    fences.append(fence)
                    execution.abort(candidate)
                candidate = execution.begin()
                execution.register(candidate)
                storage = execution.private_storage(candidate)
                with self.assertRaises(Rejected):
                    execution.start_probe(candidate, storage)
                for _ in range(32):
                    execution.update()
                    execution.complete_host(execution.claim_host())
                self.assertEqual(len(execution.retained_probes), depth)
                self.assertEqual(execution.completed_host_reads, 32)
                execution.signal(fences[0], -5)
                execution.reap_known_completions()
                execution.start_probe(candidate, storage)
                self.assertEqual(len(execution.retained_probes), depth - 1)

    def test_lost_reply_does_not_make_abort_undo_activation(self):
        execution = Execution()
        candidate, fence = self.prepared(execution)
        execution.signal(fence)
        receipt = execution.commit(candidate)
        execution.update(gpu_only=True, configuration=object())
        self.assertEqual(execution.commit(candidate), receipt)
        self.assertEqual(execution.abort(candidate), "active")
        self.assertIs(execution.active, candidate)
        self.assertEqual(execution.generation, receipt)

    def test_capability_publication_precedes_gpu_only_updates(self):
        execution = Execution()
        candidate, fence = self.prepared(execution)
        with self.assertRaises(Rejected):
            execution.update(gpu_only=True)
        self.assertEqual(execution.content, 0)
        execution.signal(fence)
        execution.commit(candidate)
        execution.update(gpu_only=True)
        self.assertEqual(execution.content, 1)

    def test_accepted_host_read_retires_after_activation(self):
        execution = Execution()
        old = execution.claim_host()
        candidate, fence = self.prepared(execution)
        execution.signal(fence)
        execution.commit(candidate)
        with self.assertRaises(Rejected):
            execution.claim_host()
        self.assertIn(old, execution.host_reads)
        execution.complete_host(old)
        self.assertEqual(execution.host_reads, set())

    def test_another_output_cannot_commit_or_abort_a_candidate(self):
        execution = Execution()
        other = Execution()
        candidate, fence = self.prepared(execution)
        execution.signal(fence)
        for operation in (other.commit, other.abort, other.claim_live):
            with self.assertRaises(Rejected):
                operation(candidate)
        self.assertIs(execution.candidate, candidate)
        execution.commit(candidate)

    def test_accepted_content_update_finishes_before_activation(self):
        execution = Execution()
        candidate, fence = self.prepared(execution)
        execution.signal(fence)
        execution.accept_update(execution.check_update())
        with self.assertRaises(Rejected):
            execution.commit(candidate)
        execution.install_update()
        execution.commit(candidate)
        self.assertEqual(execution.content, 1)

    def test_accepted_modeset_makes_the_candidate_stale_when_installed(self):
        execution = Execution()
        candidate, fence = self.prepared(execution)
        execution.signal(fence)
        execution.accept_update(execution.check_update(configuration=object()))
        with self.assertRaises(Rejected):
            execution.commit(candidate)
        execution.install_update()
        with self.assertRaises(Rejected):
            execution.commit(candidate)
        self.assertEqual(execution.mode, "host")

    def test_unaccepted_host_update_remains_valid_after_activation(self):
        execution = Execution()
        update = execution.check_update()
        candidate, fence = self.prepared(execution)
        execution.signal(fence)
        execution.commit(candidate)
        execution.accept_update(update)
        execution.install_update()
        self.assertEqual(execution.mode, "gpu")
        self.assertEqual(execution.content, 1)

    def test_another_outputs_update_cannot_block_activation(self):
        execution = Execution()
        candidate, fence = self.prepared(execution)
        execution.signal(fence)
        with self.assertRaises(Rejected):
            execution.accept_update(Execution().check_update())
        self.assertIsNone(execution.pending)
        execution.commit(candidate)


if __name__ == "__main__":
    unittest.main()
