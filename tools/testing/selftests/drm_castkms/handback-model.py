#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Model orderly return from GPU execution to the built-in renderer.

Start with an authorized, active GPU worker. All decisions are serialized;
installation and scene publication remain separate events. One update may be
accepted or awaiting publication at a time. This bounded scheduler is not a
claim about DRM queue depth or a proof of the kernel's locking implementation.
Native submissions are a trusted-worker oracle, not a GPU access-control gate.
"""

from dataclasses import dataclass, field
import unittest


class Rejected(Exception):
    pass


@dataclass(eq=False)
class Fence:
    status: int | None = None


@dataclass(eq=False)
class Claim:
    submitted: list = field(default_factory=list)
    released: bool = False


@dataclass(frozen=True)
class Scene:
    configuration: object
    gpu_only: bool
    serial: int


@dataclass(eq=False)
class Token:
    owner: object
    worker: object
    authority: int
    origin: object
    phase: str = "requested"
    target: object = None
    gate_epoch: int | None = None
    receipt: int | None = None


@dataclass(frozen=True)
class Update:
    owner: object
    configuration: object
    gpu_only: bool
    token: Token | None


class Handback:
    def __init__(self):
        self.worker = object()
        self.worker_alive = True
        self.authority = 1
        self.worker_authority = self.authority
        self.mode = "gpu"
        self.epoch = 1
        self.generation = 2
        self.gate = False
        self.installed = Scene(object(), True, 1)
        self.published = self.installed
        self.pending = None
        self.publication = None
        self.token = None
        self.tokens = set()
        self.claims = set()

    @staticmethod
    def require(condition):
        if not condition:
            raise Rejected()

    def known(self, token):
        self.require(token.owner is self and token in self.tokens)

    def authorized(self, token):
        self.known(token)
        self.require(self.worker_alive and token.worker is self.worker)
        self.require(token.authority == self.authority)

    def current(self, token):
        self.authorized(token)
        self.require(token is self.token)
        if token.phase == "requested":
            self.require(token.origin is self.installed.configuration)
        else:
            self.require(token.phase == "draining" and self.gate)
            self.require(token.target is self.installed.configuration)
            self.require(token.gate_epoch == self.epoch)

    def request_host(self, *, allocation_failure=False):
        self.require(self.mode == "gpu" and self.worker_alive and self.token is None)
        self.require(self.worker_authority == self.authority)
        self.require(not allocation_failure)
        token = Token(self, self.worker, self.authority, self.installed.configuration)
        self.tokens.add(token)
        self.token = token
        return token

    def validate_update(self, update):
        self.require(update.owner is self and self.mode != "lost")
        self.require(not update.gpu_only or (self.mode == "gpu" and not self.gate))
        if update.token is not None:
            self.current(update.token)
            self.require(update.token.phase == "requested" and not update.gpu_only)

    def check_update(self, *, gpu_only=False, configuration=None, token=None):
        update = Update(self, configuration or self.installed.configuration, gpu_only, token)
        self.validate_update(update)
        return update

    def accept_update(self, update):
        self.require(self.pending is None and self.publication is None)
        self.validate_update(update)
        self.pending = update

    def install_update(self):
        self.require(self.pending is not None)
        update = self.pending
        self.validate_update(update)
        # Installation owns the gate and target-configuration transition together.
        self.installed = Scene(update.configuration, update.gpu_only,
                               self.installed.serial + 1)
        self.pending = None
        self.publication = self.installed
        if update.token is not None:
            token = update.token
            self.epoch += 1
            self.gate = True
            self.mode = "draining"
            token.phase = "draining"
            token.target = self.installed.configuration
            token.gate_epoch = self.epoch

    def fail_update(self):
        self.require(self.pending is not None)
        self.pending = None

    def publish_scene(self):
        self.require(self.publication is not None)
        self.published = self.publication
        self.publication = None

    def update(self, **kwargs):
        self.accept_update(self.check_update(**kwargs))
        self.install_update()
        self.publish_scene()

    def compose_host(self, token, *, failure=False):
        self.current(token)
        self.require(token.phase == "draining")
        self.require(not self.published.gpu_only)
        self.require(self.published.configuration is token.target)
        self.require(not failure)
        return self.published

    def publish_host(self, token, image):
        self.authorized(token)
        if token.phase == "published":
            return token.receipt
        self.current(token)
        self.require(token.phase == "draining")
        self.require(self.pending is None and self.publication is None)
        self.require(not self.installed.gpu_only and not self.published.gpu_only)
        self.require(self.published.configuration is token.target)
        self.require(not image.gpu_only and image.configuration is token.target)
        self.mode = "host"
        self.gate = False
        self.epoch += 1
        self.generation += 1
        token.phase = "published"
        token.receipt = self.generation
        return token.receipt

    def abort_host(self, token):
        self.known(token)
        if token.phase in ("published", "canceled"):
            return token.phase
        self.authorized(token)
        self.require(token is self.token)
        if self.gate:
            self.epoch += 1
        self.gate = False
        self.mode = "gpu"
        token.phase = "canceled"
        self.token = None
        return token.phase

    def claim_gpu(self):
        self.require(self.worker_alive and self.mode in ("gpu", "draining"))
        self.require(self.worker_authority == self.authority)
        claim = Claim()
        self.claims.add(claim)
        return claim

    def submit(self, claim):
        self.require(claim in self.claims and not claim.released and self.worker_alive)
        fence = Fence()
        claim.submitted.append(fence)
        return fence

    def release(self, claim, fences):
        self.require(claim in self.claims and not claim.released)
        self.require(len(fences) == len(claim.submitted))
        self.require(set(fences) == set(claim.submitted))
        claim.released = True

    def acknowledge_handback(self, token):
        self.authorized(token)
        self.require(token.phase == "published" and self.mode == "host")
        self.require(all(claim.released for claim in self.claims))
        return token.receipt

    def worker_lost(self):
        self.worker_alive = False
        self.mode = "lost"


class HandbackTests(unittest.TestCase):
    def draining(self):
        execution = Handback()
        token = execution.request_host()
        execution.update(token=token, configuration=object())
        return execution, token

    def test_request_and_test_only_leave_gpu_eligibility_unchanged(self):
        execution = Handback()
        original = execution.installed
        token = execution.request_host()
        execution.check_update(token=token, configuration=object())
        self.assertFalse(execution.gate)
        self.assertEqual(execution.epoch, 1)
        self.assertIs(execution.installed, original)
        execution.check_update(gpu_only=True)
        execution.claim_gpu()

    def test_failed_reservation_does_not_publish_a_request(self):
        execution = Handback()
        with self.assertRaises(Rejected):
            execution.request_host(allocation_failure=True)
        self.assertIsNone(execution.token)
        self.assertEqual(execution.mode, "gpu")

    def test_tagged_installation_binds_the_new_configuration(self):
        execution, token = self.draining()
        self.assertIsNot(token.origin, token.target)
        self.assertIs(token.target, execution.installed.configuration)
        self.assertEqual(token.gate_epoch, execution.epoch)
        self.assertEqual(execution.mode, "draining")
        with self.assertRaises(Rejected):
            execution.check_update(gpu_only=True)
        execution.claim_gpu()
        image = execution.compose_host(token)
        execution.publish_host(token, image)

    def test_gate_does_not_authorize_reading_the_old_published_scene(self):
        execution = Handback()
        token = execution.request_host()
        execution.accept_update(execution.check_update(token=token))
        execution.install_update()
        self.assertTrue(execution.gate)
        self.assertTrue(execution.published.gpu_only)
        with self.assertRaises(Rejected):
            execution.compose_host(token)
        execution.publish_scene()
        execution.compose_host(token)

    def test_an_accepted_predecessor_finishes_before_the_tagged_update(self):
        execution = Handback()
        token = execution.request_host()
        tagged = execution.check_update(token=token)
        execution.accept_update(execution.check_update(gpu_only=True))
        with self.assertRaises(Rejected):
            execution.accept_update(tagged)
        execution.install_update()
        with self.assertRaises(Rejected):
            execution.accept_update(tagged)
        execution.publish_scene()
        execution.accept_update(tagged)
        execution.install_update()
        execution.publish_scene()
        execution.compose_host(token)

    def test_unaccepted_gpu_update_must_revalidate_against_the_gate(self):
        execution = Handback()
        gpu_update = execution.check_update(gpu_only=True)
        token = execution.request_host()
        execution.update(token=token)
        with self.assertRaises(Rejected):
            execution.accept_update(gpu_update)
        self.assertIsNone(execution.pending)

    def test_a_predecessor_modeset_invalidates_the_original_request(self):
        execution = Handback()
        token = execution.request_host()
        tagged = execution.check_update(token=token)
        execution.update(gpu_only=True, configuration=object())
        with self.assertRaises(Rejected):
            execution.accept_update(tagged)
        self.assertFalse(execution.gate)
        execution.abort_host(token)
        execution.request_host()

    def test_failed_tagged_update_keeps_gpu_execution(self):
        execution = Handback()
        token = execution.request_host()
        original = execution.installed
        execution.accept_update(execution.check_update(token=token))
        execution.fail_update()
        self.assertIs(execution.installed, original)
        self.assertFalse(execution.gate)
        self.assertEqual(token.phase, "requested")

    def test_cancel_between_acceptance_and_installation_rejects_the_tag(self):
        execution = Handback()
        token = execution.request_host()
        original = execution.installed
        execution.accept_update(execution.check_update(token=token))
        execution.abort_host(token)
        with self.assertRaises(Rejected):
            execution.install_update()
        execution.fail_update()
        self.assertIs(execution.installed, original)
        self.assertFalse(execution.gate)

    def test_cancel_lifts_the_gate_without_restoring_an_old_scene(self):
        execution, token = self.draining()
        image = execution.compose_host(token)
        current = execution.installed
        epoch = execution.epoch
        execution.abort_host(token)
        self.assertIs(execution.installed, current)
        self.assertIs(execution.published, current)
        self.assertGreater(execution.epoch, epoch)
        execution.check_update(gpu_only=True)
        with self.assertRaises(Rejected):
            execution.publish_host(token, image)

    def test_configuration_change_rejects_a_late_host_image(self):
        execution, token = self.draining()
        image = execution.compose_host(token)
        execution.update(configuration=object())
        current = execution.installed
        with self.assertRaises(Rejected):
            execution.publish_host(token, image)
        execution.abort_host(token)
        self.assertIs(execution.installed, current)
        self.assertEqual(execution.mode, "gpu")

    def test_failed_host_composition_allows_cancellation_without_rollback(self):
        execution, token = self.draining()
        current = execution.installed
        with self.assertRaises(Rejected):
            execution.compose_host(token, failure=True)
        self.assertEqual(execution.mode, "draining")
        execution.abort_host(token)
        self.assertIs(execution.installed, current)
        execution.claim_gpu()

    def test_old_cancellation_or_image_cannot_change_a_replacement_request(self):
        execution, old = self.draining()
        image = execution.compose_host(old)
        execution.abort_host(old)
        replacement = execution.request_host()
        execution.update(token=replacement)
        self.assertEqual(execution.abort_host(old), "canceled")
        with self.assertRaises(Rejected):
            execution.publish_host(old, image)
        self.assertIs(execution.token, replacement)
        self.assertTrue(execution.gate)
        execution.publish_host(replacement, execution.compose_host(replacement))

    def test_content_updates_do_not_starve_host_publication(self):
        for frames in (1, 2, 8, 32):
            with self.subTest(frames=frames):
                execution, token = self.draining()
                image = execution.compose_host(token)
                for _ in range(frames):
                    execution.update()
                execution.publish_host(token, image)
                self.assertLess(image.serial, execution.published.serial)
                self.assertEqual(execution.mode, "host")

    def test_pending_content_update_finishes_before_host_publication(self):
        execution, token = self.draining()
        image = execution.compose_host(token)
        execution.accept_update(execution.check_update())
        with self.assertRaises(Rejected):
            execution.publish_host(token, image)
        execution.install_update()
        with self.assertRaises(Rejected):
            execution.publish_host(token, image)
        execution.publish_scene()
        execution.publish_host(token, image)

    def test_host_publication_stops_new_claims_before_release_acknowledgment(self):
        execution, token = self.draining()
        claim = execution.claim_gpu()
        image = execution.compose_host(token)
        receipt = execution.publish_host(token, image)
        with self.assertRaises(Rejected):
            execution.claim_gpu()
        with self.assertRaises(Rejected):
            execution.acknowledge_handback(token)
        fence = execution.submit(claim)
        execution.release(claim, [fence])
        self.assertIsNone(fence.status)
        self.assertEqual(execution.acknowledge_handback(token), receipt)

    def test_incomplete_release_cannot_acknowledge_handback(self):
        execution, token = self.draining()
        claim = execution.claim_gpu()
        first = execution.submit(claim)
        second = execution.submit(claim)
        execution.publish_host(token, execution.compose_host(token))
        for incomplete in ([], [first], [first, first]):
            with self.assertRaises(Rejected):
                execution.release(claim, incomplete)
            with self.assertRaises(Rejected):
                execution.acknowledge_handback(token)
        execution.release(claim, [second, first])
        execution.acknowledge_handback(token)

    def test_native_completion_does_not_replace_the_release_promise(self):
        execution, token = self.draining()
        claim = execution.claim_gpu()
        fence = execution.submit(claim)
        fence.status = -5
        execution.publish_host(token, execution.compose_host(token))
        with self.assertRaises(Rejected):
            execution.acknowledge_handback(token)
        execution.release(claim, [fence])
        execution.acknowledge_handback(token)

    def test_lost_reply_does_not_let_abort_reactivate_gpu_execution(self):
        execution, token = self.draining()
        image = execution.compose_host(token)
        receipt = execution.publish_host(token, image)
        execution.update(configuration=object())
        self.assertEqual(execution.publish_host(token, image), receipt)
        self.assertEqual(execution.abort_host(token), "published")
        self.assertEqual(execution.mode, "host")
        self.assertEqual(execution.acknowledge_handback(token), receipt)

    def test_foreign_output_cannot_use_the_handback_token(self):
        execution, token = self.draining()
        other = Handback()
        image = execution.compose_host(token)
        for operation in (lambda: other.check_update(token=token),
                          lambda: other.abort_host(token),
                          lambda: other.publish_host(token, image)):
            with self.assertRaises(Rejected):
                operation()
        self.assertEqual(execution.mode, "draining")

    def test_authority_loss_cannot_restore_the_workers_permission(self):
        execution, token = self.draining()
        image = execution.compose_host(token)
        execution.authority += 1
        for operation in (lambda: execution.publish_host(token, image),
                          lambda: execution.abort_host(token), execution.claim_gpu):
            with self.assertRaises(Rejected):
                operation()

    def test_worker_identity_change_rejects_the_old_token(self):
        execution, token = self.draining()
        image = execution.compose_host(token)
        execution.worker = object()
        with self.assertRaises(Rejected):
            execution.publish_host(token, image)
        with self.assertRaises(Rejected):
            execution.abort_host(token)
        self.assertTrue(execution.gate)

    def test_worker_loss_before_acknowledgment_is_not_orderly_handback(self):
        for published in (False, True):
            with self.subTest(published=published):
                execution, token = self.draining()
                claim = execution.claim_gpu()
                fence = execution.submit(claim)
                if published:
                    execution.publish_host(token, execution.compose_host(token))
                execution.worker_lost()
                with self.assertRaises(Rejected):
                    execution.acknowledge_handback(token)
                if published:
                    self.assertEqual(execution.abort_host(token), "published")
                else:
                    with self.assertRaises(Rejected):
                        execution.abort_host(token)
                self.assertEqual(execution.mode, "lost")
                self.assertFalse(claim.released)
                self.assertIsNone(fence.status)


if __name__ == "__main__":
    unittest.main()
