#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0
"""Join the source and destination models without merging their authorities.

The grant factory stands in for a trusted policy decision. Scope labels are
not DRM rights evaluation. All operations are serialized model decisions.
"""

from dataclasses import dataclass, field
import importlib.util
from itertools import permutations
from pathlib import Path
import sys
import unittest


def load_model(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


source = load_model("preparation_source_model", "preparation-model.py")
output = load_model("preparation_output_model", "output-model.py")
Rejected = source.Rejected


@dataclass(frozen=True)
class CaptureGrant:
    owner: object
    output: int
    destination: output.Grant


@dataclass
class Stage:
    scope: str
    writes: list = field(default_factory=list)


class Pipeline:
    def __init__(self, outputs=1, staging_depth=2):
        self.source = source.Model(outputs, staging_depth)
        self.output = output.OutputModel()
        self.stages = {}
        self.cache = {}

    def grant(self, output_id, recipient_domain):
        self.source.require(output_id in self.source.current)
        scope = f"output {output_id}, recipient {recipient_domain}"
        return CaptureGrant(self, output_id, output.Grant(scope))

    def claim_source(self, grant, output_id):
        self.source.require(grant.owner is self and grant.output == output_id)
        self.source.require(grant.destination.live)
        claim = self.source.claim_source(output_id)
        self.stages[claim] = Stage(grant.destination.scope)
        return claim

    def claim_output(self, claim, grant, allocation):
        self.source.require(grant.owner is self)
        stage = self.stages[claim]
        self.source.require(stage.scope == grant.destination.scope)
        self.source.require(self.source.staging_status(claim) == 0)
        # Pixels stand for the private image, not a reread of current scanout.
        pixels = f"picture {self.source.claims[claim].scene}"
        try:
            write = self.output.claim(grant.destination, allocation, pixels)
        except output.Rejected as error:
            raise Rejected() from error
        stage.writes.append(write)
        return write

    def recycle_stage(self, claim):
        # E may be recycled after its E-to-D uses finish. Consumers of D do
        # not retain E, and no destination operation adds an A reader fence.
        self.source.require(all(write.completed for write in self.stages[claim].writes))
        self.source.recycle_staging(claim, downstream_done=True)
        self.cache = {scope: cached for scope, cached in self.cache.items()
                      if cached != claim}

    def remember_stage(self, claim, grant):
        self.source.require(grant.owner is self and grant.destination.live)
        self.source.require(self.stages[claim].scope == grant.destination.scope)
        self.source.require(self.source.claims[claim].scene == self.source.current[grant.output])
        self.source.require(self.source.staging_status(claim) == 0)
        self.cache[grant.destination.scope] = claim

    def cached_stage(self, grant):
        self.source.require(grant.owner is self and grant.destination.live)
        claim = self.cache.get(grant.destination.scope)
        if claim is None:
            return None
        if self.source.claims[claim].scene != self.source.current[grant.output]:
            return None
        return claim if self.source.staging_status(claim) == 0 else None


class PipelineTests(unittest.TestCase):
    def current_stage(self, pipeline, grant):
        pipeline.source.queue()
        claim = pipeline.claim_source(grant, grant.output)
        copy = pipeline.source.submit_source(claim)
        pipeline.source.release(claim, [copy])
        pipeline.source.signal(copy)
        return claim

    def test_cache_reuse_does_not_add_a_source_reader(self):
        pipeline = Pipeline()
        grant = pipeline.grant(0, "X")
        allocation = pipeline.output.allocate(grant.destination)
        claim = self.current_stage(pipeline, grant)
        pipeline.remember_stage(claim, grant)
        old = pipeline.source.current[0]
        readers = frozenset(pipeline.source.scenes[old].fences)
        ticket = pipeline.source.prepare([0])
        self.assertTrue(pipeline.source.ready(ticket))
        for _ in range(3):
            cached = pipeline.cached_stage(grant)
            self.assertEqual(cached, claim)
            write = pipeline.claim_output(cached, grant, allocation)
            pipeline.output.submit(write)
            pipeline.output.complete(write)
        self.assertEqual(len(pipeline.source.claims), 1)
        self.assertFalse(pipeline.source.scenes[old].claims)
        self.assertEqual(pipeline.source.scenes[old].fences, readers)
        pipeline.source.complete_commit(pipeline.source.accept(ticket))
        self.assertIsNone(pipeline.cached_stage(grant))
        pipeline.recycle_stage(claim)
        self.assertFalse(pipeline.cache)

    def test_same_framebuffer_recommit_invalidates_cached_content(self):
        for replace_producer in (False, True):
            with self.subTest(replace_producer=replace_producer):
                pipeline = Pipeline()
                grant = pipeline.grant(0, "X")
                allocation = pipeline.output.allocate(grant.destination)
                framebuffer = object()
                producer = pipeline.source.submit_native()
                pipeline.source.signal(producer)
                pictures = []
                old_claim = None
                for update in range(2):
                    if update and replace_producer:
                        producer = pipeline.source.submit_native()
                        pipeline.source.signal(producer)
                    ticket = pipeline.source.prepare([0])
                    # Each row explicitly selects the same framebuffer and
                    # geometry, with no damage hint claiming unchanged pixels.
                    request = pipeline.source.capture_request([[0, 7, 8, 0]],
                                                              {7: framebuffer}, {8: producer},
                                                              {9: ticket}, 9)
                    pipeline.source.complete_commit(pipeline.source.accept_request(request))
                    self.assertIs(request.changes[0].framebuffer, framebuffer)
                    self.assertIsNone(pipeline.cached_stage(grant))
                    claim = self.current_stage(pipeline, grant)
                    pipeline.remember_stage(claim, grant)
                    if old_claim is not None:
                        with self.assertRaises(Rejected):
                            pipeline.remember_stage(old_claim, grant)
                        with self.assertRaises(Rejected):
                            pipeline.source.submit_source(old_claim)
                        self.assertEqual(pipeline.cached_stage(grant), claim)
                    write = pipeline.claim_output(pipeline.cached_stage(grant), grant, allocation)
                    pipeline.output.submit(write)
                    pipeline.output.complete(write)
                    pictures.append(allocation.pixels)
                    old_claim = claim
                self.assertNotEqual(*pictures)

    def test_cache_rejects_invalid_or_incompatible_images(self):
        pipeline = Pipeline()
        grant = pipeline.grant(0, "X")
        other = pipeline.grant(0, "Y")
        pipeline.source.queue()
        claim = pipeline.claim_source(grant, 0)
        copy = pipeline.source.submit_source(claim)
        with self.assertRaises(Rejected):
            pipeline.remember_stage(claim, grant)
        pipeline.source.release(claim, [copy])
        pipeline.source.signal(copy, -5)
        with self.assertRaises(Rejected):
            pipeline.remember_stage(claim, grant)
        pipeline.recycle_stage(claim)
        valid = self.current_stage(pipeline, grant)
        with self.assertRaises(Rejected):
            pipeline.remember_stage(valid, other)
        pipeline.remember_stage(valid, grant)
        self.assertIsNone(pipeline.cached_stage(other))
        pipeline.output.revoke(grant.destination)
        with self.assertRaises(Rejected):
            pipeline.cached_stage(grant)
        pipeline.recycle_stage(valid)
        self.assertFalse(pipeline.cache)

    def test_output_failure_does_not_invalidate_the_private_source_image(self):
        pipeline = Pipeline(staging_depth=1)
        grant = pipeline.grant(0, "X")
        allocation = pipeline.output.allocate(grant.destination)
        claim = self.stage(pipeline, grant)
        old = pipeline.source.claims[claim].scene
        write = pipeline.claim_output(claim, grant, allocation)
        pipeline.output.submit(write)
        self.assertFalse(pipeline.output.complete(write, status=-5))
        self.assertEqual(pipeline.output.results.query(write.result), -5)
        self.assertEqual(pipeline.source.staging_status(claim), 0)
        self.assertTrue(pipeline.source.scenes[old].retired)
        self.assertEqual(allocation.pixels, "uncertain")
        retry = pipeline.claim_output(claim, grant, allocation)
        pipeline.output.submit(retry)
        self.assertTrue(pipeline.output.complete(retry))
        self.assertEqual(allocation.pixels, f"picture {old}")
        pipeline.recycle_stage(claim)
        # Cleanup did not require acknowledging either terminal result.
        self.assertEqual(len(pipeline.output.results.results), 2)

    def test_pre_revoke_source_claim_can_finish_after_revocation(self):
        pipeline = Pipeline()
        grant = pipeline.grant(0, "X")
        pipeline.source.queue()
        claim = pipeline.claim_source(grant, 0)
        pipeline.output.revoke(grant.destination)
        copy = pipeline.source.submit_source(claim)
        pipeline.source.release(claim, [copy])
        ticket = pipeline.source.prepare([0])
        self.assertTrue(pipeline.source.ready(ticket))
        pipeline.source.signal(copy)
        pipeline.source.complete_commit(pipeline.source.accept(ticket))
        self.assertEqual(pipeline.source.staging_status(claim), 0)

    def stage(self, pipeline, grant):
        pipeline.source.queue()
        claim = pipeline.claim_source(grant, grant.output)
        copy = pipeline.source.submit_source(claim)
        pipeline.source.release(claim, [copy])
        commit = pipeline.source.accept(pipeline.source.prepare([grant.output]))
        pipeline.source.signal(copy)
        pipeline.source.complete_commit(commit)
        return claim

    def test_source_claim_checks_live_grant_and_requested_output(self):
        pipeline = Pipeline(outputs=2)
        grant = pipeline.grant(0, "X")
        foreign = Pipeline(outputs=2).grant(0, "X")
        pipeline.source.queue()
        for candidate, output_id in ((grant, 1), (foreign, 0)):
            with self.assertRaises(Rejected):
                pipeline.claim_source(candidate, output_id)
        pipeline.output.revoke(grant.destination)
        with self.assertRaises(Rejected):
            pipeline.claim_source(grant, 0)
        self.assertEqual(pipeline.source.demand, 1)
        self.assertFalse(pipeline.source.claims)
        self.assertFalse(pipeline.stages)

    def test_pre_revoke_source_claim_does_not_authorize_later_output(self):
        pipeline = Pipeline()
        grant = pipeline.grant(0, "X")
        allocation = pipeline.output.allocate(grant.destination)
        pipeline.source.queue()
        claim = pipeline.claim_source(grant, 0)
        pipeline.output.revoke(grant.destination)
        copy = pipeline.source.submit_source(claim)
        pipeline.source.release(claim, [copy])
        commit = pipeline.source.accept(pipeline.source.prepare([0]))
        pipeline.source.signal(copy)
        pipeline.source.complete_commit(commit)
        self.assertEqual(pipeline.source.staging_status(claim), 0)
        with self.assertRaises(Rejected):
            pipeline.claim_output(claim, grant, allocation)
        self.assertEqual(allocation.pixels, "cleared")
        self.assertFalse(allocation.busy)
        pipeline.recycle_stage(claim)

    def test_output_claim_retains_private_image_until_write_completion(self):
        pipeline = Pipeline()
        grant = pipeline.grant(0, "X")
        allocation = pipeline.output.allocate(grant.destination)
        claim = self.stage(pipeline, grant)
        old = pipeline.source.claims[claim].scene
        write = pipeline.claim_output(claim, grant, allocation)
        pipeline.output.revoke(grant.destination)
        self.assertTrue(pipeline.source.scenes[old].retired)
        with self.assertRaises(Rejected):
            pipeline.recycle_stage(claim)
        pipeline.output.submit(write)
        with self.assertRaises(Rejected):
            pipeline.recycle_stage(claim)
        self.assertFalse(pipeline.output.complete(write))
        self.assertEqual(allocation.pixels, f"picture {old}")
        pipeline.recycle_stage(claim)
        with self.assertRaises(Rejected):
            pipeline.claim_output(claim, grant, allocation)

    def test_busy_destination_does_not_hold_the_source(self):
        pipeline = Pipeline(staging_depth=1)
        grant = pipeline.grant(0, "X")
        allocation = pipeline.output.allocate(grant.destination)
        previous = pipeline.output.claim(grant.destination, allocation, "previous")
        pipeline.output.submit(previous)
        claim = self.stage(pipeline, grant)
        old = pipeline.source.claims[claim].scene
        with self.assertRaises(Rejected):
            pipeline.claim_output(claim, grant, allocation)
        self.assertTrue(pipeline.source.scenes[old].retired)
        pipeline.source.queue()
        with self.assertRaises(Rejected):
            pipeline.claim_source(grant, 0)
        for _ in range(10):
            pipeline.source.complete_commit(pipeline.source.accept(pipeline.source.prepare([0])))
        self.assertEqual(pipeline.source.demand, 1)
        self.assertFalse(pipeline.source.scenes[pipeline.source.current[0]].claims)
        pipeline.output.complete(previous)
        write = pipeline.claim_output(claim, grant, allocation)
        pipeline.output.submit(write)
        self.assertTrue(pipeline.output.complete(write))
        self.assertEqual(allocation.pixels, f"picture {old}")
        pipeline.recycle_stage(claim)
        pipeline.claim_source(grant, 0)

    def test_new_scope_cannot_reuse_old_staging_or_exported_storage(self):
        pipeline = Pipeline()
        old_grant = pipeline.grant(0, "X")
        retained = pipeline.output.allocate(old_grant.destination)
        old_claim = self.stage(pipeline, old_grant)
        pipeline.output.revoke(old_grant.destination)
        new_grant = pipeline.grant(0, "Y")
        fresh = pipeline.output.allocate(new_grant.destination)
        with self.assertRaises(Rejected):
            pipeline.claim_output(old_claim, new_grant, fresh)
        new_claim = self.stage(pipeline, new_grant)
        with self.assertRaises(Rejected):
            pipeline.claim_output(new_claim, new_grant, retained)
        write = pipeline.claim_output(new_claim, new_grant, fresh)
        pipeline.output.submit(write)
        pipeline.output.complete(write)
        self.assertEqual(retained.pixels, "cleared")
        self.assertNotEqual(fresh.pixels, "cleared")

    def test_invalid_staging_cannot_acquire_destination_write(self):
        pipeline = Pipeline()
        grant = pipeline.grant(0, "X")
        allocation = pipeline.output.allocate(grant.destination)
        pipeline.source.queue()
        claim = pipeline.claim_source(grant, 0)
        copy = pipeline.source.submit_source(claim)
        pipeline.source.release(claim, [copy])
        with self.assertRaises(Rejected):
            pipeline.claim_output(claim, grant, allocation)
        pipeline.source.signal(copy, -5)
        with self.assertRaises(Rejected):
            pipeline.claim_output(claim, grant, allocation)
        self.assertFalse(allocation.busy)
        self.assertFalse(pipeline.stages[claim].writes)

    def test_successful_copy_of_failed_producer_cannot_publish(self):
        pipeline = Pipeline()
        grant = pipeline.grant(0, "X")
        allocation = pipeline.output.allocate(grant.destination)
        producer = pipeline.source.submit_native()
        ticket = pipeline.source.prepare([0])
        request = pipeline.source.capture_request([[0, 7, 8, 0]], {7: "frame"},
                                                  {8: producer}, {9: ticket}, 9)
        commit = pipeline.source.accept_request(request)
        pipeline.source.signal(producer, -5)
        pipeline.source.complete_commit(commit)
        claim = self.stage(pipeline, grant)
        self.assertTrue(all(pipeline.source.native[fence] == 0
                            for fence in pipeline.source.claims[claim].submitted))
        with self.assertRaises(Rejected):
            pipeline.claim_output(claim, grant, allocation)
        self.assertEqual(allocation.pixels, "cleared")
        self.assertFalse(allocation.busy)
        pipeline.recycle_stage(claim)

    def test_output_claim_revoke_submit_complete_orders(self):
        completed = denied = 0
        for schedule in permutations(("claim", "revoke", "submit", "complete")):
            with self.subTest(schedule=schedule):
                pipeline = Pipeline()
                grant = pipeline.grant(0, "X")
                allocation = pipeline.output.allocate(grant.destination)
                claim = self.stage(pipeline, grant)
                write = None
                for event in schedule:
                    try:
                        if event == "claim":
                            write = pipeline.claim_output(claim, grant, allocation)
                        elif event == "revoke":
                            pipeline.output.revoke(grant.destination)
                        elif event == "submit" and write is not None:
                            pipeline.output.submit(write)
                        elif event == "complete" and write is not None:
                            pipeline.output.complete(write)
                            completed += 1
                    except (Rejected, output.Rejected):
                        self.assertIn(event, ("claim", "complete"))
                        denied += 1
                    self.assertEqual(allocation.busy, write is not None and not write.completed)
                    if allocation.pixels != "cleared":
                        self.assertIsNotNone(write)
                        self.assertTrue(write.completed)
                        self.assertLess(schedule.index("claim"), schedule.index("revoke"))
        self.assertGreater(completed, 0)
        self.assertGreater(denied, 0)

    def test_unacknowledged_results_do_not_retain_source_or_private_image(self):
        pipeline = Pipeline(staging_depth=1)
        grant = pipeline.grant(0, "X")
        allocation = pipeline.output.allocate(grant.destination)
        claim = self.stage(pipeline, grant)
        writes = []
        for _ in range(pipeline.output.results.capacity):
            write = pipeline.claim_output(claim, grant, allocation)
            pipeline.output.submit(write)
            pipeline.output.complete(write, notify=False)
            writes.append(write)
        pipeline.recycle_stage(claim)
        self.assertTrue(pipeline.source.scenes[pipeline.source.claims[claim].scene].retired)
        next_claim = self.stage(pipeline, grant)
        with self.assertRaises(Rejected):
            pipeline.claim_output(next_claim, grant, allocation)
        self.assertFalse(allocation.busy)
        self.assertFalse(pipeline.stages[next_claim].writes)
        self.assertEqual(len(pipeline.output.results.results), pipeline.output.results.capacity)
        for _ in range(10):
            pipeline.source.complete_commit(pipeline.source.accept(pipeline.source.prepare([0])))
        pipeline.output.results.acknowledge(writes[0].result)
        write = pipeline.claim_output(next_claim, grant, allocation)
        pipeline.output.submit(write)
        pipeline.output.complete(write)
        pipeline.recycle_stage(next_claim)

    def test_closed_results_do_not_release_private_image_before_native_end(self):
        pipeline = Pipeline(staging_depth=1)
        grant = pipeline.grant(0, "X")
        allocation = pipeline.output.allocate(grant.destination)
        claim = self.stage(pipeline, grant)
        write = pipeline.claim_output(claim, grant, allocation)
        pipeline.output.submit(write)
        pipeline.output.results.close()
        with self.assertRaises(Rejected):
            pipeline.recycle_stage(claim)
        self.assertFalse(pipeline.output.complete(write))
        pipeline.recycle_stage(claim)
        self.assertFalse(pipeline.output.results.results)


if __name__ == "__main__":
    unittest.main()
