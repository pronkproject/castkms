#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0
"""Join the source and destination models without merging their authorities.

The grant factory stands in for a trusted policy decision. Scope labels are
not DRM rights evaluation. All operations are serialized model decisions.
"""

from dataclasses import dataclass
import importlib.util
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


class Pipeline:
    def __init__(self, outputs=1, staging_depth=2):
        self.source = source.Model(outputs, staging_depth)
        self.output = output.OutputModel()
        self.stages = {}

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


class PipelineTests(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
