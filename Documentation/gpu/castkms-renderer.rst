========================================
CastKMS negotiated renderer protocol
========================================

Integration boundary
====================

The Rust CastKMS renderer protocol in ``include/uapi/drm/castkms_drm.h`` is
ready for coordinated Pronk/compositor integration at renderer version 8,
capability encoding version 2 and complete-scene encoding version 1. These
are experimental driver interfaces, not a claim of upstream ABI acceptance.
Use the header from the same revision; reject unsupported versions instead
of guessing layouts. The executable example is
``tools/testing/selftests/drm_castkms/renderer-control.c``.

Renderer endpoints require separate output-scoped authority. Public image
capture authority does not authorize raw source export or renderer control.
The KMS client still owns scene allocation and atomic updates; the renderer
does not acquire modesetting authority through its endpoint. Pronk must
coordinate the transition token with that client.

Use raw ``ioctl()`` (retrying ``EINTR`` as appropriate) for anonymous renderer
files: pending activation may return ``EAGAIN`` and requires returning to the
event loop, not libdrm's unconditional retry. For the ordinary DRM atomic
commit, a pending ``PREPARE_FD`` returns ``EBUSY`` without consuming the
ticket. ``drmModeAtomicCommit()`` may be used; wait/query before retrying.
Never echo ``PREPARE_FD`` readback into an atomic property dump/restore.

Registration flags and reserved words must be zero. Monitor creation is
input-only and returns descriptors through an explicit ``files`` pointer.
Use ``DEQUEUE_SCENE`` for all scenes, including a single primary layer.
Capability constant names distinguish ``KIND_*``, ``PROFILE_*``, ``FORMAT_*``
and ``STATE_*`` fields. YUV capability masks use the named bits derived from
the scene's ``DRM_CASTKMS_YUV_ENCODING_*`` and ``DRM_CASTKMS_YUV_RANGE_*`` values.

Profiles and queries
====================

Source and output dimensions have independent inclusive minimum and maximum
bounds. All renderer bounds must be positive and ordered on both axes. Pronk's
fixed-size private pool should set ``min_output == max_output == {width, height}``;
it need not restrict source framebuffers to that size. Source bounds describe
whole framebuffers, not cropped regions. Set their minima to ``{1, 1}`` if
smaller source buffers are supported. Replacing the pool with a different size
requires a new negotiated profile, not silently changing the active contract.

``QUERY_CAPABILITIES`` returns one coherent snapshot of execution, active
contract, optional pending contract, transition token and validation epoch.
It works with disabled video while output authority remains live. Allocate
``DRM_CASTKMS_CAPABILITY_QUERY_MAX_BYTES`` or provide at least the 72-byte
header: ``ENOSPC`` writes the header with the required total size. A retry
observes a fresh snapshot, not a reservation of the earlier one. Copy faults
may partially write output. Validate version, size and offsets before use.

The native-endian profile consists of a 128-byte header and at most 256
32-byte storage records. Unknown flags and reserved fields must be zero.

* ``HOST`` is canonical: version and kind are set, all remaining bytes are
  zero, and there are no storage records. It names the fixed CPU policy,
  not an empty renderer format set.
* ``RENDERER`` describes a supported whole scene: output/source dimensions,
  crop, fractional coordinates, positioning, source/destination scale
  ratios, layer/role counts, color operations and LUT bounds. Limits apply
  to every role; advertise the intersection of per-role restrictions.
* Each storage record is an exact fourcc/modifier/memory-plane-count tuple,
  with native/imported provenance and per-plane alignment/pitch bounds.
  Implicit layout and explicit LINEAR are distinct. At least one provenance
  flag is required, alignments are positive powers of two, and duplicate
  tuples are rejected. A declaration is not proof of working GPU import.

Current bounds are 24 layers, four memory planes per framebuffer, 16 color
operations per pipeline and 256 LUT entries. Sampling, premultiplied
source-over blending and stable stacking have the complete-scene protocol's
fixed semantics; the renderer must implement them, not reinterpret them.

Keep identities separate:

====================== ==================================================
Identity               Meaning
====================== ==================================================
candidate_id           Endpoint-local startup/retry identity
execution generation   Output execution publication identity
capability generation  Immutable output contract identity; not content
validation epoch       Changes with gate installation/removal/activation
transition token       Device-scoped, non-reused pending transition name
scene/job identities   Content observation and retained source-read claim
====================== ==================================================

Canceled capability generations are not reused. None of these returned
values grants continuing authority. Do not compare counters from different
outputs as if they described one globally atomic activation.

Transition workflow
===================

1. Query current execution. On an authorized endpoint, ``BEGIN_TAKEOVER``
   with that execution generation reserves one candidate for the output.
   The old renderer stays active. BEGIN currently requires enabled video.
   Use a fresh endpoint when replacing an active negotiated worker, even
   when both endpoints belong to the same process.
2. Register the bounded target through ``REGISTER_PROFILE``. GPU targets
   require successful private/startup probe completion before activation;
   probes do not authorize raw current-source reads. HOST targets need no
   probe. Registration leaves ordinary KMS acceptance unchanged.
3. Give the returned transition token to the KMS client. It chooses a
   complete scene supported by both contracts, allocating/redrawing buffers
   if needed, and includes the token in the CRTC's ``CASTKMS_TRANSITION``
   property on an ordinary atomic update. Other properties and producer
   synchronization follow normal KMS rules.
4. Successful installation places that output under the intersection of
   both contracts. ``TEST_ONLY`` installs nothing. The transition property
   always reads as zero and is never inherited by subsequent requests.
   Compatible animation continues without retagging or pinning a content
   serial. A tagged modeset binds the transition to its target configuration;
   an unrelated configuration change cancels it on successful installation.
5. ``COMMIT_TAKEOVER`` requires a registered profile, the gate and publication of the
   installed scene (or a compatible successor), current authority and GPU
   readiness where applicable. It atomically publishes execution and the
   active contract, lifts the gate and closes new old-worker admission.
   ``EINVAL`` means the candidate has no registered profile. ``EAGAIN`` means
   readiness/publication is not complete; query and retry.
6. Drain previously admitted reads on their original endpoint. Neither
   activation nor a notification releases them. New scenes now use the
   new contract, including GPU-only layouts where negotiated.

One atomic request may tag several outputs: native installation validates
the complete cohort. Renderer activation is per output, not an all-output
transaction. Keep separate endpoints, generations and drain accounting.

If the contracts have no suitable visible scene in common, BEGIN while the
old output is enabled, then explicitly disable it in the tagged atomic
update. Activate with no source-bearing scene, and re-enable with buffers
accepted by the target. This is a visible interruption, never an implicit
blank or a kernel conversion of an existing framebuffer.

For orderly HOST handback, register the canonical HOST profile on a fresh
endpoint and follow exactly the same tag/commit sequence. Keep the old GPU
endpoint alive until its outstanding reads are resolved. HOST reads obey
normal native preparation and retained read completion; handback does not
assert that all old GPU work has finished.

Retries, cancellation and loss
========================================

Registration commits before copying its reply. On ``EFAULT``, query the
pending snapshot to recover the token; blindly registering again returns
``EBUSY``. BEGIN instead publishes its candidate only after successful
reply copying. Failed scene dequeue installs no descriptors, including
after a partial metadata copy.

Repeating successful COMMIT for the same current candidate reconciles a
lost reply. Query execution and capabilities after ambiguous outcomes.
Successful activation is not undone by ABORT: returning to another profile
requires another transition. ABORT or closing a pending endpoint cancels
the proposal/gate, leaving the active contract and current scene in place;
it never restores an old framebuffer. Revocation and display-authority
changes invalidate pending transitions. Stale identities are rejected.

Hotplug notifications are reprobe hints, not a counted event stream or
authorization. Always query current state. There is no automatic timeout;
userspace should bound its attempts and abort abandoned candidates.

Unexpected active-worker/device loss is not orderly handback. Automatic
HOST fallback and recovery from the lost-worker state are not implemented.
Do not promise crash-transparent migration or release uncertain GPU reads
merely because a process, endpoint or device disappeared.

Complete scenes and read lifetime
========================================

Use ``DEQUEUE_SCENE`` and ``DRM_CASTKMS_RENDERER_SCENE_MAX_BYTES``. The stream
includes primary, overlays and cursor in back-to-front order, source crop,
destination geometry, memory-plane descriptors and ordered plane/output
color records. The producer sync_file covers all layers and must complete
successfully before any source read. There is at most one outstanding job
per endpoint; ``EBUSY`` requires releasing it and ``ENODATA`` means there is
no new source-bearing scene.

``RELEASE_SOURCE`` resolves the whole job. ``NO_ACCESS`` promises no source
access occurred; ``CPU_DONE`` promises CPU access and coherency operations
ended; ``SUBMITTED`` supplies a native sync_file covering every submitted
source read and promises no later submission under that job. Keeping an
ordinary DMA-BUF descriptor does not authorize further reads. Private
render targets and encoder/capture destinations have separate lifetimes.

Source-stage content evidence
-----------------------------

The kernel source-job provider can retain content evidence after release:
the original producer records, native render completion, content serial,
output, configuration, owner and execution identity. It retains neither a
framebuffer nor a source-read claim. Keeping the evidence therefore does
not postpone source retirement or require the displayed content to remain
unchanged.

Pixel validity requires successful completion of every recorded producer
and the render operation. A producer error is terminal for validity even
while native access is still pending; it does not permit early storage
reuse. Admission under the evidence rechecks live renderer authority and
rejects a different output, configuration or execution incarnation.

These are internal provider records, not userspace receipt handles. The
renderer endpoint discards them on release. Evidence alone grants no capture
authority and names no private allocation.

Private images
--------------

The kernel renderer provider can register private backing allocations and
reserve one for a source-to-private-image job. It claims the source only
after that independent storage and its cleanup record are available. The
job's completion must cover both source reads and private-image writes.
Its result binds the original content evidence to the reserved storage.
Keeping that result prevents overwrite without holding a source-read claim.

The trusted renderer owns the native format interpretation and must verify
that its layout fits the supplied allocations. The kernel neither maps the
image nor applies the HOST compositor's linear-format restrictions. All
backing allocations must remain private to the rendering service, including
imports and aliases. They must not be exported to an encoder or capture
recipient. Native queue, mapping and reservation dependencies must also
exclude downstream release waits from source-reading work; kernel pool
bookkeeping alone cannot prove that property.

Registration tracks known aliases by DMA-BUF and reservation identity. It
rejects overlap with current source storage again at source admission.
Device-wide limits of 128 images and 512 MiB include registrations retained
after their caller drops its handle. Private use names are increasing and
never reused. Removing a handle does not bypass a pending native use or
the allocation accounting. An abandoned claim with unknown completion
quarantines its storage and reports a failed reuse attempt, rather than
pretending the allocation is available. Recovery from that fault is not
implemented.

Private-image registration and bound rendering are kernel-provider APIs.
The renderer file does not expose them. Destination claims and delegated
output delivery remain unimplemented.

GPU envelope and remaining work
========================================

Primary/overlay static formats include the CPU formats plus binary16 RGB;
the KMS dimension envelope is 16384 by 16384. Cursor remains ARGB8888 with
a 512 by 512 bound. The active contract and any gate are authoritative:
the static envelope alone grants no admission.

``IN_FORMATS`` currently advertises a LINEAR baseline, not the complete
negotiated modifier ceiling. CastKMS's tuple callback permits other modifier
metadata within the static format set; the renderer capability query and
atomic validation determine actual negotiated acceptance. Integration must
explicitly consume that driver protocol. Hotplug does not rewrite or extend
``IN_FORMATS``. Native DRM framebuffer representation still bounds possible
layouts; arbitrary vendor auxiliary/compression planes are not implemented
merely by declaring a modifier.

HOST remains linear and CPU-mappable, with an 8192 dimension ceiling and
checked allocation/layout bounds. GPU support does not require teaching the
kernel compositor tiled or floating-point pixel decoding. The kernel tests
exercise tiled/float metadata admission, HOST rejection and a 9000-pixel
mode; they do not render those pixels on a GPU.

Renderer integration requires profile discovery/registration, compositor token
coordination, complete-scene import/composition and retained-read release.
Actual cross-GPU import/render qualification, delegated final-image capture
delivery and lost-worker recovery remain separate work. The passing fake
renderer is protocol evidence, not a
completed GPU capture/encoding pipeline.
