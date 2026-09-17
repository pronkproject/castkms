========================================
CastKMS negotiated renderer protocol
========================================

Integration boundary
====================

The Rust CastKMS renderer protocol in ``include/uapi/drm/castkms_drm.h`` is
ready for coordinated Pronk/compositor integration at renderer version 9,
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

Register renderer-private storage with ``REGISTER_IMAGE``, then supply its
``image_id`` to ``DEQUEUE_SCENE`` and allocate
``DRM_CASTKMS_RENDERER_SCENE_MAX_BYTES`` for the result. The stream
includes primary, overlays and cursor in back-to-front order, source crop,
destination geometry, memory-plane descriptors and ordered plane/output
color records. The producer sync_file covers all layers and must complete
successfully before any source read. There is at most one outstanding source
job per endpoint; ``EBUSY`` also covers a private image still retained by native
work or output reads. ``ENODATA`` means there is no unconsumed source-bearing
scene.
Failed descriptor publication installs no descriptors, admits no userspace
access, and permits retry with the same image name.

``RELEASE_SOURCE`` resolves the whole source-to-private job. ``NO_ACCESS``
promises neither source nor private-image access occurred and produces no
private image. The same scene remains eligible for retry under a new job ID,
without requiring another KMS update. Retry still checks current authority
and source-read admission: it cannot bypass a preparation hold or permanent
seal. ``CPU_DONE`` promises CPU access and coherency operations
ended; ``SUBMITTED`` supplies a native sync_file covering every submitted
source read and private-image write and promises no later submission under
that job. Keeping an
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
renderer endpoint retains successful source-stage reports with their private
image, including pending or failed native completion. Evidence alone grants
no capture authority; output admission separately checks pixel validity.

Private images
--------------

``REGISTER_IMAGE`` retains one to four distinct readable/writable DMA-BUFs
under a positive, increasing endpoint-local image name. Before activation,
register a renderer profile first; image dimensions must satisfy its inclusive
output bounds and may differ from the current mode. After activation, dimensions
must match the active output. Failed registration does not consume the name.
Registering storage neither accesses pixels nor excludes arbitrary external
submissions, establishes renderer readiness or authorizes live source access.

``DEQUEUE_SCENE`` reserves the selected image for a source-to-private job.
It withdraws any retained content from that image before attempting reuse.
Withdrawal does not end outstanding native accesses. It claims the source only
after that independent storage and its cleanup record are available. The
job's completion must cover both source reads and private-image writes.
Its result binds the original content evidence to the reserved storage.
Keeping that result prevents overwrite without holding a source-read claim.

``UNREGISTER_IMAGE`` removes a name without making it reusable. A publishing,
claimed or releasing source job returns ``EBUSY``. After source release,
removal is allowed while submitted native work retains its own storage and
budget. Success is not a native-completion or buffer-reuse signal. Cleanup
remains available after renderer revocation.

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
HOST capture destinations participate in the same alias ledger, including
after namespace removal while detached delivery retains their storage.
Private and recipient pools each have independent device-wide
limits of 128 images and 512 MiB, including registrations retained after
their caller drops its handle. Their common alias ledger does not let one
pool consume the other's accounting credits. Private use names are increasing and
never reused. Removing a handle does not bypass a pending native use or
the allocation accounting. An abandoned claim with unknown completion
quarantines its storage and reports a failed reuse attempt, rather than
pretending the allocation is available. Recovery from that fault is not
implemented.

The renderer file exposes registration and bound source jobs. Private images
are retained by name for output-stage admission, not exported as capture results.

Delegated output stages
-----------------------

The kernel capture provider registers recipient destinations under an exact
capture grant, configuration and execution generation. Its initial output
layout is complete linear XRGB8888 rows; that output subset does not restrict
the renderer's private-image layout or negotiated source formats/modifiers.
Destination dimensions follow the GPU envelope rather than the HOST layout
ceiling. Registration checks writable access, row bounds and known aliases
against private storage and current compositor sources. Source overlap is
checked again when a write is claimed.

An exported DMA-BUF is not revocable. Trusted importers must supply backing
compatible with every prior recipient of that allocation and use fresh
backing across incompatible grants or audiences. Renaming a pool or creating
another DMA-BUF wrapper does not establish fresh storage. The common ledger
rejects known live aliases but cannot detect arbitrary descriptor forwarding
or all exporter-specific aliases. The output worker must initialize exposed
padding, unused channel bits and allocation regions; registration alone does
not sanitize pixels.

Each queue assigns strictly increasing request names within its own stream
incarnation. Destination storage has independent exclusive-use ownership:
once native access retires, another stream may reuse that destination even
if its request name is smaller or a terminal result remains unacknowledged.
Queued demand retains neither a source claim nor a private frame.
Explicit reuse fences and initially acquired
implicit dependencies retain their individual status. Pending dependencies
are distinct from completed errors, including EAGAIN. External users must
stop submitting new destination work before reservation; observing fences
does not establish native exclusion. A pending destination leaves the request
source-unbound and does not acquire private storage from an attempted claim.

Output admission requires successful private-image production and destination
reuse, plus live renderer and capture authority under one stabilized display
scope. Cleanup storage is allocated before admitting the bounded private-to-
recipient stage. The admitted claim exposes borrowed views of its exact private
input and checked destination, without exposing request internals to the worker.
Its native completion covers private reads and recipient writes, never compositor
source retirement. CPU completion includes cache maintenance; a no-access report
promises that neither allocation was touched.

Closing handles or canceling a submitted request does not release either
allocation before native completion. Revocation rejects new claims. An already
claimed stage may submit while cancellation is being resolved, but a revoked
in-flight request produces an error after native retirement, not a successful
new frame. Requests retain the concrete output fence once submission is
reported, including after cancellation or revocation. It is cleanup evidence,
not independent permission to publish a frame, and its success does not
override a terminal request error. No future-work fence is manufactured.
Successful output metadata retains the private image's production time:
the latest original producer/source fence signaling time, or the CPU
completion report time when applicable. Repeated output from the same
private image preserves that timestamp; recipient writes and dequeue do
not create a new image-production time. Failed output carries no timestamp.
Completion reconciliation can lag native fence signaling. Once
reconciled, a request has one terminal result; cleanup queries do not authorize
new frame publication. The stage releases its hold on private storage
independently of result dequeue or the recipient's later use. Unreported
claimed access is explicitly lost and quarantined, not normal completion
or permission to reuse storage.

The provider supports at most sixteen live queues per device and eight
requests per queue, including unacknowledged terminal results. Native work
retains its accounting after queue removal. Per-output route ownership
publishes a bounded recipient directory with round-robin selection; observing
the route or a queue does not keep the renderer's active ownership alive.
Selection skips recipients with pending reuse or busy metadata publication.
Closing discovery does not wait behind a recipient's userspace copyout:
the current queue operation reconciles cancellation after unlocking.

Renderer sessions have independent source and output publication slots.
An output claim reserves only its admitted private image and destination;
source animation can continue while an earlier output write remains pending.
Failed output publication releases with no access. Published access retains
both allocations until the worker reports how access ended and any submitted
native work has retired.
Lost native access returns ``EIO`` without publishing or acknowledging a
completed result; it never implies that the destination can be reused.

Advisory wakeups cover scene changes, authority loss, reuse completion,
private-image production and output retirement. Recipient operations notify
again after unlocking so a worker that skipped a busy queue can retry.
Observers register before checking authoritative state. Detaching a fence
observation does not complete native work or release its storage ownership.

Public output-job transport, delegated capture-file integration, negotiated
destination layouts and real GPU rendering remain unimplemented. The public
capture endpoint still uses HOST delivery; renderer scene jobs bind registered
private images but do not yet publish recipient-output claims.

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
