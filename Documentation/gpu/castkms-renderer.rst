===================================
CastKMS renderer constraints protocol
===================================

Integration boundary
====================

The experimental interface in ``include/uapi/drm/castkms_drm.h`` uses version 1
for the renderer endpoint, renderer constraints and complete-scene encoding.
Use the header from the same revision and reject unsupported versions.
The executable fake-worker example is
``tools/testing/selftests/drm_castkms/renderer-control.c``.

Renderer authority is output-scoped and distinct from final-image capture
authority. The issuing DRM file must be the exact current master controlling
the CRTC and connector. The returned close-on-exec renderer descriptor grants
neither modesetting nor capture authority. The separate revocation descriptor
controls admission. Issuer close also revokes the grant.

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

Each renderer file owns one immutable draft and at most one published offer.
The file identifies the draft; there is no separate draft handle. Independent
files can prepare replacement workers concurrently, including with disabled
video. Preparation acquires no live source pixels and changes no KMS state.

1. ``PREPARE_OFFER`` supplies bounded renderer constraints and exact private
   pool width and height. Failure leaves an empty endpoint retryable.
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
256 fixed 32-byte format records. Their kind is
``DRM_CASTKMS_RENDERER_CONSTRAINTS_KIND``. Fixed default
constraints come from generic KMS listing, not a worker declaration.
Unknown flags and reserved fields must be zero.

Inclusive minimum and maximum dimensions describe full source framebuffers
and composed output images independently. Equal bounds express exact size.
The declared output interval must contain the private pool dimensions;
the published offer is narrowed to that exact target. Changing dimensions
requires a separately prepared endpoint.

The constraints bound crop, fractional coordinates, positioning, scale ratios,
layer and role counts, LUT lengths and color operations. Limits apply to
every role; advertise the intersection of per-role restrictions. Each format
record names an exact fourcc/modifier/memory-plane-count tuple, native/imported
provenance, and alignment/pitch bounds. Implicit layout is distinct from
explicit LINEAR. A declaration proves neither import compatibility nor access.

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
Master-loss recovery and native read retirement follow the generic constraints
contract. A list-change event only prompts re-query; it grants no authority.

Remaining integration
=====================

The public source stream composes into renderer-private storage. Public
recipient-output job transport, delegated capture-file integration, negotiated
destination layouts and real cross-GPU rendering qualification remain separate
work. Public final-image capture uses HOST delivery and rejects renderer-backed
scenes. Passing fake-worker tests is not evidence of a completed GPU
capture/encoding pipeline.
