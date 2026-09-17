.. SPDX-License-Identifier: GPL-2.0-only

=====================================
CastKMS renderer constraints protocol
=====================================

Integration boundary
====================

The experimental interface in ``include/uapi/drm/castkms_drm.h`` uses version 1
for the renderer endpoint, renderer constraints and complete-scene encoding.
Use the header from the same revision and reject unsupported versions.
The executable fake-worker example is
``tools/testing/selftests/drm_castkms/renderer-control.c``.

Renderer authority is output-scoped and distinct from final-image capture
authority. Normal issuance requires the exact current master file controlling
the CRTC and connector. An administrative helper can instead set
``DRM_CASTKMS_RENDERER_CREATE_ADMIN`` while holding ``CAP_SYS_ADMIN`` in the
initial user namespace. That explicit mode binds to the independently observed
current top-level owner interval; it does not use the helper's DRM-master
association, take master from the compositor or grant modesetting rights. The
helper must drop an accidentally acquired master role before requesting it.
The returned close-on-exec renderer descriptor grants neither modesetting nor
capture authority. The separate revocation descriptor controls admission.
Issuer close also revokes the grant.

The descriptor is bound to that ``drm_master`` identity, not permanently to
the uninterrupted interval in which it was issued. While the bound master is
absent or another master is current, control operations fail with ``EACCES``.
If the same master becomes current again, the descriptor resumes with an empty
generation after outstanding old jobs have been released. Drafts, offers,
private registrations and jobs from the earlier interval never reactivate.
An administratively issued descriptor is narrower: owner-interval loss makes
it permanently stale, including if the same ``drm_master`` later returns. The
helper must issue a fresh endpoint for the new interval.

The KMS client discovers immutable descriptions with
``DRM_IOCTL_MODE_LIST_CONSTRAINTS``, subscribes with
``DRM_CLIENT_CAP_KMS_CONSTRAINTS`` after enabling atomic support, and selects
``CONSTRAINTS_ID`` in ordinary atomic state. See
``include/uapi/drm/drm_constraints.h`` for listing and notification semantics.
Publication is not selection; accepted state is not proof of presentation or
GPU completion. No renderer ioctl returns ``EAGAIN`` for readiness.
A caller must handle ``EBUSY`` and ``ENODATA`` without busy-waiting.
A pending atomic ``PREPARE_FD`` returns ``EBUSY``; normal libdrm atomic
wrappers may be used. Never echo that request-only property's readback.

Private preparation
===================

Each renderer file owns one immutable draft and at most one published offer per
uninterrupted master interval. The file identifies the draft; there is no
separate draft handle. Independent files can prepare replacement workers
concurrently within an interval, including with disabled video. A same-master
reacquisition may reuse a drained file for a fresh generation. Preparation
acquires no live source pixels and changes no KMS state.

1. ``PREPARE_OFFER`` supplies bounded renderer constraints and exact private
   pool width and height. A declaration with no allocation intersection with
   the output's plane topology returns ``EOPNOTSUPP``. Failure leaves an empty
   endpoint retryable.
2. ``REGISTER_IMAGE`` supplies increasing positive image names and one to
   four distinct read/write DMA-BUFs each. Dimensions must equal the draft's
   pool dimensions. The trusted worker validates native layouts and imports.
3. ``SUBMIT_PROBE`` reports completed CPU-only private work with fd -1, or
   supplies a materialized native sync_file for independently submitted work.
   The probe carries no display content and must not depend on future
   userspace submissions or recipient release.
4. ``PUBLISH_OFFER`` requires registered storage and successful probe
   completion. Pending work returns ``EBUSY``; terminal probe failure
   returns ``EREMOTEIO``, with the native status retained on the sync_file.
   The result contains the positive native constraints ID.
5. The KMS client selects that ID alongside compatible buffers, geometry,
   color and synchronization state, with ``DRM_MODE_ATOMIC_ALLOW_MODESET``.
   ``TEST_ONLY`` neither reserves readiness nor selects the worker.

The publication reply is copied before native listing. Any failed operation
leaves no new selectable entry; ignore partial output. Successful publication
pins the complete private registration set and cannot be extended.
Repeating publication returns ``EALREADY``; ``QUERY`` reconciles its identity
without publishing again. Query state is advisory and requires live issuer
authority. A published offer need not be selected.

Whole-scene declarations
========================

Renderer constraints consist of a 128-byte native-endian header and up to
256 fixed 48-byte format records. Their kind is
``DRM_CASTKMS_RENDERER_CONSTRAINTS_KIND``. Fixed default
constraints come from generic KMS listing, not a worker declaration.
Unknown flags and reserved fields must be zero.

Inclusive minimum and maximum dimensions describe full source framebuffers
and composed output images independently. Equal bounds express exact size.
The declared output interval must contain the private pool dimensions;
the published offer is narrowed to that exact target. Changing dimensions
requires a separately prepared endpoint.

The constraints bound crop, fractional coordinates, positioning, scale ratios,
layer and role counts, LUT lengths and color operations. The operation ceiling
applies independently to each plane pipeline and the output pipeline. Limits
apply to every role; advertise the intersection of per-role restrictions. Each
format record names an exact fourcc/modifier/memory-plane-count tuple,
native/imported provenance, framebuffer width/height alignment and byte
alignment/minimum/maximum pitch bounds. Implicit layout is distinct from explicit LINEAR. A
declaration proves neither import compatibility nor access. Tuples without an
aligned size in the source bounds, or which cannot fit DRM's generic minimum
pitch at their smallest aligned source width, are omitted; an offer with no
usable tuple is rejected before private preparation.
When scaling is absent, both source-to-destination ratio bounds are exactly
1.0 in unsigned 16.16 representation.
Published generic KMS constraints carry the same per-format allocation limits,
so a compositor can choose storage before selecting the offer. Per-plane
geometry records expose cropping, fractional source coordinates, destination
position and scale ratios for every plane with allocation choices. Scalar
rules expose usable YUV encoding and range values with YUV-plane applicability,
including for declarations that mix RGB and YUV allocations.
Overlapping active-plane limits express both the declaration's total layer
ceiling and any narrower role ceiling across the actual planes for that output.
Cross-plane geometry relationships and color-pipeline contents cannot be
represented as independent records, so complete atomic validation remains
authoritative for the whole scene.

The static KMS envelope is 16384 by 16384 with primary/overlay formats
including binary16 RGB. Cursor remains ARGB8888, at most 512 by 512.
``IN_FORMATS`` provides a LINEAR baseline; generic constraints descriptions
and complete atomic validation determine actual acceptance. GPU-only layouts
do not inherit the CPU compositor's linear-only ceiling. Native DRM
framebuffer representation still limits possible auxiliary/compression planes.
HOST requires checked CPU-readable linear storage, at most 8192 per axis.

Source jobs
===========

``DEQUEUE_SCENE`` names a registered private image and provides
``DRM_CASTKMS_RENDERER_SCENE_MAX_BYTES`` writable bytes. It reserves one
source-to-private job, retaining the exact accepted constraints ID, content
serial, buffers and producer completion. Only the selected live worker can
acquire source access. There is one outstanding job per endpoint.

The scene includes primary, overlays and cursor in stable back-to-front order,
source crop, destination geometry and ordered plane/output color operations.
Sampling is nearest-neighbor; blending is premultiplied source-over against
opaque black. A producer already completed with an error makes dequeue return
``EREMOTEIO`` without publishing files or a source claim; its native error is
not a queue-readiness result. The failed scene is discarded, so polling becomes
idle and dequeue returns ``ENODATA`` until a new scene is accepted. The producer
fence covers all layers and must complete successfully before reading. Each
exported descriptor is close-on-exec.
Failed metadata copy or admission installs no descriptors and permits retry.

``poll`` is an advisory dequeue prompt, not a reservation or completion
signal. An unselected offer is idle. An outstanding job or busy private image
returns ``EBUSY``; no changed scene returns ``ENODATA``. Withdrawal reports
``POLLHUP|POLLERR`` without resolving outstanding access.

``RELEASE_SOURCE`` reports one of:

* ``NO_ACCESS``: neither source nor private-image access occurred. The same
  scene can be retried with a new job ID if authority and admission remain live.
* ``CPU_DONE``: all CPU accesses and coherency operations ended.
* ``SUBMITTED``: a materialized native sync_file covers every source read
  and private-image write, with no later submission under that job.

Repeating the latest accepted release succeeds. Ordinary DMA-BUF descriptors
do not authorize reads after release. Successful reporting does not mean a
pending native fence has completed. Retained image evidence preserves original
content and producer identity without retaining a source claim.

Private storage must remain independent of sources and recipients. Alias
tracking rejects known overlapping DMA-BUF/reservation identities. The renderer
must additionally isolate queues and mappings from downstream release waits.
Pool accounting alone cannot establish native dependency independence.

Recipient output jobs
=====================

Final-image clients use the generic capture interface in
``include/uapi/drm/drm_capture.h``. While a renderer constraints entry is
accepted, ``DESCRIBE`` returns that exact worker's output configuration,
``CREATE_STREAM`` registers bounded demand with the worker, and
``QUEUE_OUTPUT`` supplies recipient-owned storage plus its reuse fence. A
stream never grants access to the KMS source planes or renderer-private image.

After ``RELEASE_SOURCE`` has produced a valid private image, the renderer calls
``DEQUEUE_OUTPUT`` with its private image name. Success returns one writable
recipient DMA-BUF and checked layout under a new output job ID. Version 1
destinations are single-plane linear XRGB8888. A destination allocation and
its described image span may each be at most 512 MiB, and all endpoints share
a device-wide 512 MiB recipient ledger. The claim waits independently
for the private-image completion and recipient reuse dependency; it does not
reacquire or retain a compositor source read. Failed copyout installs no fd and
releases both images without access.

``RELEASE_OUTPUT`` uses the same ``NO_ACCESS``, ``CPU_DONE`` and ``SUBMITTED``
meanings as source release, but its fence covers private-image reads and
recipient writes only. Capture publishes a terminal result after that native
work completes. Cancellation suppresses a successful result but cannot revoke
an already claimed write. Renderer loss quarantines an unresolved destination
rather than fabricating completion. ``poll`` reports either a changed scene or
a ready recipient claim; it remains advisory and never reserves the job.

Withdrawal and replacement
==========================

``WITHDRAW_OFFER`` idempotently stops selection and new admission. It does
not change accepted KMS state, restore HOST, or complete native accesses.
Revoker close and issuer close revoke admission while the renderer file still
accepts outstanding release and private-name cleanup. Final renderer-file close
ends that reporting channel; unreported access is not fabricated as complete.

A live offer pins its registered images. After withdrawal, unregister still
returns ``EBUSY`` for a publishing, claimed or releasing job. Submitted
native work retains storage independently after namespace removal.
Successful unregister is not permission to reuse storage before completion.

To return to HOST, the KMS client selects the listed fixed default with a
compatible complete atomic scene. To replace a worker, prepare and publish
another endpoint, then select it atomically. Keep old reporting channels until
their admitted work is resolved. Multi-output atomic selection uses ordinary
KMS transaction semantics, with independent endpoint read accounting.
Final acceptance retains every participating worker's readiness through the
same native state swap. Readiness contention returns ``EBUSY`` without
accepting any output; retry the complete atomic update after re-querying as
appropriate. A failed or withdrawn member never partially selects the cohort.

Unexpected worker loss is not transparent migration. A withdrawn selected
binding remains retained; its buffers must not be reinterpreted as HOST.
Master loss withdraws the old worker generation. Release its outstanding source
and recipient jobs; once drained, the retained endpoint reports ``EMPTY`` when
the same master returns and can prepare a new generation. A list-change event
only prompts re-query; it grants no authority.

Remaining integration
=====================

The public source and recipient transports now cover A-to-E and E-to-D as
independently released stages. Destination layout negotiation beyond the
version 1 linear XRGB8888 capture contract, physical cross-GPU import/rendering
qualification, and an installed userspace media pipeline remain. Passing the
fake-worker tests is not evidence of physical GPU interoperability.
