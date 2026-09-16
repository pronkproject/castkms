#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0
"""Sequential model of atomic constraints selection, not a kernel/UAPI test.

Run with Python 3; no device or privileges are required. Each method represents
one serialized decision for one independent output. Contracts deliberately use
disjoint format/modifier pairs so routing a scene to the wrong backend fails.
Allocation is independent of display validation. Acceptance pins a contract and
ready backend; ordered publication admits reads without a userspace activation
acknowledgment. Trusted claim release and native completion are separate events.

The model does not implement enumeration, wire layouts, resource allocation,
preparation tickets, shared-output constraints, or the kernel memory model.
"""

from dataclasses import dataclass, field
from itertools import permutations
import unittest


class Rejected(Exception):
    pass


@dataclass(frozen=True)
class Buffer:
    layout: tuple


@dataclass(frozen=True)
class Binding:
    identity: int
    backend: str
    layouts: frozenset


@dataclass(frozen=True)
class Update:
    binding: Binding
    buffer: Buffer


@dataclass
class Scene:
    update: Update
    published: bool = False
    superseded: bool = False
    error: str = None
    claims: list = field(default_factory=list)

    @property
    def retired(self):
        return (self.superseded and
                all(claim.released and claim.completed
                    for claim in self.claims))


@dataclass
class Claim:
    update: Update
    released: bool = False
    completed: bool = False


class Output:
    """Identity dictionaries retain entries; availability never changes meaning."""

    def __init__(self):
        self.entries = {}
        self.available = set()
        self.ready = set()
        self.failed = set()
        self.selected = None
        self.pending = []
        self.scenes = []
        self.visible = None
        self.next_id = 1
        self.owner = 1

    def offer(self, backend, layouts):
        if backend not in self.ready or backend in self.failed:
            raise Rejected("backend not ready")
        binding = Binding(self.next_id, backend, frozenset(layouts))
        self.next_id += 1
        self.entries[binding.identity] = binding
        self.available.add(binding.identity)
        if self.selected is None:
            self.selected = binding
        return binding

    def withdraw(self, identity):
        self.available.discard(identity)

    def check(self, owner, buffer, identity=None):
        if owner != self.owner:
            raise Rejected("stale owner")
        binding = self.selected if identity is None else self.entries.get(identity)
        if binding is None:
            raise Rejected("invalid constraints")
        # Withdrawing an offer cannot change the accepted persistent selection.
        if binding != self.selected and binding.identity not in self.available:
            raise Rejected("withdrawn constraints")
        if binding.backend not in self.ready or binding.backend in self.failed:
            raise Rejected("backend not ready")
        if buffer.layout not in binding.layouts:
            raise Rejected("incompatible scene")
        return Update(binding, buffer)

    def accept(self, owner, buffer, identity=None):
        # Real acceptance repeats validation; TEST_ONLY grants no reservation.
        update = self.check(owner, buffer, identity)
        scene = Scene(update)
        self.scenes.append(scene)
        self.pending.append(scene)
        self.selected = update.binding
        return scene

    def publish(self, scene):
        if not self.pending or self.pending[0] is not scene:
            raise Rejected("publication out of order")
        self.pending.pop(0)
        if self.visible is not None:
            self.visible.superseded = True
        scene.published = True
        if scene.update.binding.backend in self.failed:
            scene.error = "backend failed"
            scene.superseded = True
            self.visible = None
        else:
            self.visible = scene

    def read(self, scene, backend):
        if (self.visible is not scene or scene.error is not None or
                scene.update.binding.backend != backend):
            raise Rejected("source not available to backend")
        claim = Claim(scene.update)
        scene.claims.append(claim)
        return claim

    def fail(self, backend):
        self.failed.add(backend)
        # Failure closes admission, but is not evidence of native completion.
        for scene in self.scenes:
            if scene.update.binding.backend == backend:
                scene.error = "backend failed"
        if (self.visible is not None and
                self.visible.update.binding.backend == backend):
            self.visible.superseded = True
            self.visible = None


class ConstraintsTests(unittest.TestCase):
    def setUp(self):
        self.output = Output()
        self.output.ready.update(("linear", "tiled"))
        self.linear = self.output.offer("linear", (("XR24", "LINEAR"),))
        self.tiled = self.output.offer("tiled", (("AR24", "TILED"),))
        self.old_buffer = Buffer(("XR24", "LINEAR"))
        self.target_buffer = Buffer(("AR24", "TILED"))

    def select_target(self):
        return self.output.accept(1, self.target_buffer, self.tiled.identity)

    def test_target_buffer_exists_before_selection(self):
        self.assertEqual(self.output.selected, self.linear)
        with self.assertRaisesRegex(Rejected, "incompatible"):
            self.output.accept(1, self.target_buffer)
        scene = self.select_target()
        self.assertIs(scene.update.buffer, self.target_buffer)
        self.assertEqual(self.output.selected, self.tiled)
        self.assertIsNone(self.output.visible)

    def test_first_target_read_requires_publication_not_acknowledgment(self):
        scene = self.select_target()
        with self.assertRaisesRegex(Rejected, "not available"):
            self.output.read(scene, "tiled")
        self.output.publish(scene)
        self.assertIs(self.output.read(scene, "tiled").update, scene.update)
        with self.assertRaisesRegex(Rejected, "not available"):
            self.output.read(scene, "linear")

    def test_delayed_tail_retains_its_backend(self):
        old = self.output.accept(1, self.old_buffer)
        target = self.select_target()
        self.assertEqual(self.output.selected, self.tiled)
        with self.assertRaisesRegex(Rejected, "out of order"):
            self.output.publish(target)
        self.output.publish(old)
        claim = self.output.read(old, "linear")
        self.output.publish(target)
        self.assertEqual(claim.update.binding, self.linear)
        self.assertEqual(self.output.read(target, "tiled").update.binding,
                         self.tiled)
        self.assertFalse(old.retired)
        with self.assertRaises(Rejected):
            self.output.read(old, "linear")

    def test_target_selection_preserves_unpublished_tail_and_held_read(self):
        first = self.output.accept(1, self.old_buffer)
        self.output.publish(first)
        held = self.output.read(first, "linear")
        delayed = self.output.accept(1, self.old_buffer)
        target = self.select_target()
        self.output.withdraw(self.linear.identity)
        self.output.publish(delayed)
        delayed_read = self.output.read(delayed, "linear")
        self.output.publish(target)
        self.assertEqual(self.output.read(target, "tiled").update.binding,
                         self.tiled)
        for scene, claim in ((first, held), (delayed, delayed_read)):
            self.assertIs(claim.update.binding, self.linear)
            self.assertFalse(scene.retired)
            claim.completed = True
            self.assertFalse(scene.retired)
            claim.released = True
            self.assertTrue(scene.retired)

    def test_failed_predecessor_does_not_poison_target(self):
        delayed = self.output.accept(1, self.old_buffer)
        target = self.select_target()
        self.output.fail("linear")
        self.output.publish(delayed)
        self.output.publish(target)
        self.assertEqual(delayed.error, "backend failed")
        self.assertIsNone(target.error)
        self.assertEqual(self.output.read(target, "tiled").update.binding,
                         self.tiled)

    def test_omitted_or_repeated_id_retains_selection(self):
        first = self.select_target()
        for identity in (None, self.tiled.identity):
            scene = self.output.accept(1, self.target_buffer, identity)
            self.assertIs(scene.update.binding, first.update.binding)
        self.assertEqual(len(self.output.entries), 2)

    def test_zero_or_unknown_id_rejected_without_state_change(self):
        for identity in (0, 999):
            with self.assertRaisesRegex(Rejected, "invalid constraints"):
                self.output.accept(1, self.target_buffer, identity)
        self.assertEqual(self.output.selected, self.linear)
        self.assertFalse(self.output.pending)

    def test_test_only_does_not_select_or_reserve(self):
        self.output.check(1, self.target_buffer, self.tiled.identity)
        self.assertEqual(self.output.selected, self.linear)
        self.assertFalse(self.output.scenes)
        self.output.withdraw(self.tiled.identity)
        with self.assertRaisesRegex(Rejected, "withdrawn"):
            self.select_target()

    def test_readiness_is_rechecked_at_acceptance(self):
        self.output.check(1, self.target_buffer, self.tiled.identity)
        self.output.ready.remove("tiled")
        with self.assertRaisesRegex(Rejected, "not ready"):
            self.select_target()
        self.assertEqual(self.output.selected, self.linear)

    def test_only_ready_backends_can_offer(self):
        with self.assertRaisesRegex(Rejected, "not ready"):
            self.output.offer("unprepared", (("XR24", "LINEAR"),))
        self.assertEqual(self.output.next_id, 3)

    def test_owner_is_rechecked_at_acceptance(self):
        self.output.check(1, self.target_buffer, self.tiled.identity)
        self.output.owner += 1
        with self.assertRaisesRegex(Rejected, "stale owner"):
            self.select_target()
        self.assertFalse(self.output.pending)

    def test_withdrawal_after_acceptance_does_not_cancel_scene(self):
        scene = self.select_target()
        self.output.withdraw(self.tiled.identity)
        self.output.publish(scene)
        self.assertEqual(self.output.read(scene, "tiled").update.binding,
                         self.tiled)
        self.assertIs(self.output.accept(1, self.target_buffer).update.binding,
                      self.tiled)

    def test_reoffered_constraints_have_new_identity(self):
        self.output.withdraw(self.tiled.identity)
        replacement = self.output.offer("tiled", self.tiled.layouts)
        self.assertNotEqual(replacement.identity, self.tiled.identity)
        with self.assertRaisesRegex(Rejected, "withdrawn"):
            self.select_target()
        self.output.accept(1, self.target_buffer, replacement.identity)

    def test_failure_after_acceptance_never_falls_back(self):
        scene = self.select_target()
        self.output.fail("tiled")
        self.output.publish(scene)
        self.assertEqual(scene.error, "backend failed")
        self.assertEqual(self.output.selected, self.tiled)
        self.assertIsNone(self.output.visible)
        for backend in ("linear", "tiled"):
            with self.assertRaises(Rejected):
                self.output.read(scene, backend)

    def test_failure_does_not_complete_native_reads(self):
        scene = self.select_target()
        self.output.publish(scene)
        claim = self.output.read(scene, "tiled")
        self.output.fail("tiled")
        claim.released = True
        self.assertFalse(scene.retired)
        claim.completed = True
        self.assertTrue(scene.retired)

    def test_publication_release_completion_interleavings(self):
        # Every schedule is executable; no rejected steps count as coverage.
        for schedule in permutations(("accept", "publish", "release", "done")):
            if schedule.index("publish") < schedule.index("accept"):
                continue
            with self.subTest(schedule=schedule):
                self.setUp()
                old = self.output.accept(1, self.old_buffer)
                self.output.publish(old)
                claim = self.output.read(old, "linear")
                target = None
                seen = set()
                for operation in schedule:
                    if operation == "accept":
                        target = self.select_target()
                    elif operation == "publish":
                        self.output.publish(target)
                        self.output.read(target, "tiled")
                    elif operation == "release":
                        claim.released = True
                    else:
                        claim.completed = True
                    seen.add(operation)
                    self.assertEqual(claim.update.binding, self.linear)
                    self.assertEqual(old.retired,
                                     {"publish", "release", "done"} <= seen)


if __name__ == "__main__":
    unittest.main()
