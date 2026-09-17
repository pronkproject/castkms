.. SPDX-License-Identifier: GPL-2.0 OR MIT

========================================
Kernel-controlled atomic KMS constraints
========================================

Scope
=====

The native constraints helpers describe a bounded display configuration and
retain the backend implementing it in atomic CRTC state. A ready target and
its compatible scene are accepted together at state installation. Checking
the scene is not a reservation, and acceptance needs no later userspace
activation acknowledgment.

The implementation provides native atomic helpers and experimental read-only
listing through ``DRM_IOCTL_MODE_LIST_CONSTRAINTS``. The DRM core also has a
bounded event producer, an experimental change-event encoding and explicit
client subscription and persistent ``CONSTRAINTS_ID`` selection. CastKMS attaches
fixed HOST defaults and publishes prepared renderer offers through these helpers;
its worker protocol is described in :doc:`castkms-renderer`. Native test providers
use framebuffer metadata or private shmem allocations and CPU sampling. Their
results do not establish physical-GPU or cross-device import support.

Selecting constraints and updating scenes may include multiple outputs in one
atomic transaction. Acceptance stabilizes every affected list, validates all
selections and swaps state once. Contention returns ``-EBUSY`` without accepting
any selection; retry the whole transaction. Asynchronous plane updates are
rejected. Drivers which do not attach constraints keep their ordinary atomic
behavior.

Leased outputs retain their fixed default contract until every outstanding
lease of that output is revoked or destroyed, including leases retained by an
inactive master tree. Creating a lease of an output with nondefault accepted
constraints returns ``-EBUSY``. Selecting nondefault constraints on a leased
output also returns ``-EBUSY``. Default-contract updates and changes on other,
unleased outputs remain available. Lease creation serializes with atomic
installation, which rechecks the lease restriction even after a successful
earlier validation. Quiescing an output without changing its binding remains
possible.

Descriptions and identities
===========================

``drm_constraints_description_create()`` copies immutable allocation,
plane-geometry and scalar-property records. Inclusive nonzero output and
framebuffer dimension bounds may describe exact sizes by making each minimum
equal its maximum.
Framebuffer bounds concern allocation, not the fractional source rectangle.
Each allocation record names an existing plane and a standard DRM
format/modifier pair. It also states whether framebuffer memory may originate
on the queried DRM device, arrive through PRIME DMA-BUF import, or use either
origin. A memory plane's pitch and offset must satisfy the stated byte
alignments, and its pitch must not exceed the advertised maximum. The record
includes the format's memory-plane count so an allocator can size the complete
framebuffer. Tiled layouts are not restricted to software-compositor
capabilities. Common atomic validation checks these requirements on every
framebuffer memory plane; a missing optional GEM object is native storage, while
an attached imported GEM object is imported storage.

Plane-geometry rules state whether each plane permits cropping, fractional
source coordinates and nonzero destination positions. They also bound the
inclusive source-to-destination scale ratio independently on both axes in
unsigned 16.16 form. A plane without a geometry rule retains ordinary KMS
geometry semantics. Common atomic validation applies a present rule whenever
the plane is used and always keeps the source rectangle within its framebuffer.

Scalar rules use the native DRM range, signed-range, enum or bitmask type.
Signed bounds retain DRM's two's-complement unsigned representation. Enum
rules describe permitted values 0 through 63 by their mask bits; bitmask
rules describe permitted bits. A zero bitmask allows only zero; an enum mask
must be nonempty. Unknown rule types, duplicate object/property pairs and
malformed bounds are rejected, not interpreted as unrestricted support.

Output attachment and publication validate object membership, property
attachment, property type and bounds within existing discovery. Supported
scalar scene properties cover alpha, blending, rotation, stacking, YUV
encoding/range, scaling filters and cursor hotspots, together
with CRTC VRR, background color, scaling filter and sharpness. Read-only and
request-only properties, object IDs, blob contents and driver-private
properties are not scalar rules. Rules constrain enabled outputs and the
planes used by their scenes, not unused objects.

Active-plane limits name a nonempty set of plane object IDs and the maximum
number that may be used together. Every record applies. Overlapping groups can
therefore express an overall layer ceiling and narrower shared-resource or
plane-role ceilings without assigning generic meaning to driver plane roles.
Each named plane must belong to the output and have at least one allocation
record in the same description.

These descriptions guide allocation; they do not solve every combination.
The provider still validates complete scenes, LUT contents, color pipelines,
cropping/scaling interactions and shared resources. Descriptions neither
retain KMS objects nor grant access to their pixels.

One ``drm_constraints_domain`` spans a DRM device's lifetime and all its
outputs. Positive entry IDs are never reused in that domain. An entry retains
its immutable description, domain and provider context, including the module
needed for final release. Availability never changes an ID's meaning. The
domain's quota includes entries retained by snapshots, accepted state and
retiring work, not just currently offered entries.

The current native bounds are 6144 format records, 64 plane-geometry records,
64 scalar-property records, 64 active-plane-limit records and 64 plane IDs per
limit, and at most 64 entries per output list. Providers choose their
retained-entry quota and may choose a smaller list limit.
The experimental UAPI declares corresponding bounds in
``include/uapi/drm/drm_constraints.h``.
Format capacity accounts for per-plane expansion: the same format/modifier
alternative on ten planes consumes ten records. Large immutable descriptions
permit virtual allocation rather than requiring contiguous memory. The
32-MiB snapshot bound covers a full list of maximum-sized descriptions.

Lists and snapshots
======================

A list begins with a nonzero accepted selection. Adding an entry does not
select it or change current buffer validity. A suggestion is advisory; zero
clears it. Withdrawing an offer excludes new selection but cannot undo an
accepted scene. A withdrawn, unselected listing can be forgotten while
independent references continue retaining the entry.

Providers that publish a ready entry as their new suggestion use the combined
add-and-suggest operation. The entry and advisory target then become visible in
one generation change; failures publish neither change.

``drm_constraints_list_snapshot()`` returns a bounded immutable list with
generation, selected ID, suggested ID and availability metadata. A nonzero
expected generation must match or the call returns ``-ESTALE``. Snapshots
retain every listed entry independently of list changes or closure.
Generation changes concern entries, availability, selection and suggestions,
not ordinary repeated frames. No-op metadata operations preserve it. Identity
and generation exhaustion return ``-EOVERFLOW`` instead of wrapping.

Listing positions are not identities. Neither obtaining a snapshot nor
checking one of its entries reserves later acceptance. Callers must tolerate
withdrawal and readiness loss before the final decision.

Kernel snapshot encoding
========================

``drm_constraints_snapshot_encode()`` serializes one retained snapshot into
a kernel buffer. The native-endian prototype layout uses fixed-width fields,
explicit padding and aligned 64-bit values. List and description headers are
versioned; entries have a stride, and descriptions contain length-delimited
output-dimension, per-plane format/modifier, plane-geometry and scalar-property
records.
Every offset is relative to the start of the complete snapshot. Per-plane
format records are alternatives; geometry rules, scalar rules and active-plane
limits apply together. Unknown required records make an entry unusable, not
unrestricted.

The encoding is bounded to 32 MiB, including a maximum-size native list.
All padding and reserved output fields are zero. A null buffer with zero
capacity discovers the required size. An undersized buffer returns
``-ENOSPC`` and the required size without modifying any payload. Other errors
leave the size output unchanged. Success writes exactly the required bytes,
including when the kernel buffer is unaligned.

The snapshot retains its original generation, identities and availability
even after the list changes or closes. The serialized bytes themselves
retain no resources and grant no authority. The encoder uses the installed
UAPI structures in ``drm_constraints.h``. Userspace copyout is a separate
operation and does not hold list or provider locks while faulting.

Read-only listing
=================

``DRM_IOCTL_MODE_LIST_CONSTRAINTS`` takes ``struct drm_mode_list_constraints``
and requires the current modesetting master. CRTC lookup respects the calling
file's lease visibility. Unknown or inaccessible CRTCs return ``-ENOENT``;
outputs without constraints return ``-EOPNOTSUPP``. A closed list returns
``-ESTALE``. Discovery does not opt a file into selecting constraints or
receiving notifications, and does not change its accepted KMS contract.

Flags, padding and reserved inputs must be zero. Set both the data pointer and
capacity to zero for size discovery; otherwise both must be nonzero. A nonzero
expected generation must match the retained snapshot. Success and ``-ENOSPC``
return its generation and required size. An undersized buffer receives no
payload. Other errors provide no usable output metadata; discard any partially
copied bytes on ``-EFAULT``. Allocation and copying use the actual bounded
snapshot size, not the caller's advertised capacity.

The ioctl returns no descriptors and performs no activation. It does not use
``-EAGAIN`` and is suitable for libdrm's ordinary ``drmIoctl()`` wrapper. A
successful query reserves neither availability nor subsequent selection.

Client opt-in
=============

Enable ``DRM_CLIENT_CAP_ATOMIC`` before setting
``DRM_CLIENT_CAP_KMS_CONSTRAINTS`` to one through ``DRM_IOCTL_SET_CLIENT_CAP``.
The latter records support for constraints and subscribes the file to list
changes. Neither capability grants modesetting authority. Notifications still
require current-master identity and visible CRTCs. Atomic preparation is a
separate capability and is not implicitly enabled.

Setting the constraints capability to zero pauses notification production.
Already queued records remain readable. A current master cannot opt out while
a visible output has nondefault accepted constraints; that request returns
``-EBUSY`` without changing the subscription. Restore the fixed defaults or
relinquish modesetting ownership first. Disable constraints support before
disabling atomic support. Unsupported devices or devices with no attached
constraints output return ``-EOPNOTSUPP`` on subscription; values other than
zero or one return ``-EINVAL``.

The first successful subscription allocates one bounded producer, retained
until file closure. Repeated enable/disable cycles pause and resume that same
producer. Failed subscription does not publish events or enable the capability.
File teardown stops deferred work before disposing the remaining DRM events.
The client adapter is tested through the ordinary ioctl dispatcher.

Persistent atomic selection
===========================

Participating CRTCs expose ``DRM_CONSTRAINTS_ID_PROPERTY`` (``CONSTRAINTS_ID``),
an atomic unsigned range with positive, device-scoped entry IDs. The property
is absent from unattached outputs. Writing it requires the constraints client
capability, including when repeating the accepted ID. Zero is invalid.

Supply a listed selectable ID together with its compatible framebuffer,
geometry, color and synchronization state. A changed binding requires
``DRM_MODE_ATOMIC_ALLOW_MODESET``. Omitting the property preserves the accepted
binding; repeating it does not request a transition. Readback reports accepted
state, not presentation or GPU completion. ``TEST_ONLY`` neither changes that
readback nor reserves target availability. Actual installation rechecks the
entry and complete scene; withdrawn targets return ``ESTALE``.

Preparation resolves the identifier into an owned entry reference before
waiting. Retries use that exact reference, not a fresh numeric lookup, and
recheck client opt-in and modesetting authority. Retention does not preserve
availability. A fully disabled, plane-free update may repeat its accepted ID
after list closure, allowing ordinary persistent-property clients to quiesce
a failed output. It cannot select another binding through the closed list.

Bounded change notifications
===========================

``DRM_EVENT_KMS_CONSTRAINTS_LIST_CHANGED`` carries a CRTC ID, list generation
and flags in a 32-byte record on the ordinary DRM event stream. A zero flags
field carries an observed nonzero generation. ``DRM_KMS_CONSTRAINTS_LIST_CLOSED``
carries generation zero and means that further listing is permanently stale.
The reserved field is zero. An event prompts a fresh query; it neither selects
an entry nor signals presentation or native execution completion.

The core producer reserves one reusable event slot per attached output and
never rewrites a queued payload. Further changes coalesce behind that record.
After consumption, the latest generation is delivered if it differs from the
one sent. Queue exhaustion retains the pending change, and returned capacity
triggers another attempt without polling or allocating another slot. Outputs
are retried round-robin so a frequently changing output cannot monopolize the
available notification capacity. Failed and short reads retain the event.

Publication rechecks current-master identity and CRTC registration/lease
visibility. Loss of authority excludes new notifications; previously queued
records remain historical observations. Observer destruction detaches list and
event-space waiters and joins deferred work before releasing the file context.
Queued event slots independently retain their storage until consumed or
discarded during file teardown.

Read-only listing does not subscribe a file. Clients must
query at initial setup and resume, and their DRM event dispatch must expose
the complete record instead of silently consuming an unknown event type.

Native atomic integration
=========================

The provider calls ``drm_constraints_device_init()`` before creating CRTCs or
registering the device. ``drm_constraints_crtc_init()`` attaches a validated,
provider-prepared default to a disabled, unregistered CRTC. The provider is
responsible for default readiness; attachment validates metadata and scope.
Only drivers using the common CRTC-state lifetime and atomic installation
helpers may attach it. New offers
use ``drm_constraints_crtc_add()`` so object and property scope is checked
before publication.

Ordinary plane format/modifier discovery remains unchanged. Every offered
allocation must be supported by its referenced plane, and the allocation,
import and framebuffer-construction path must admit target buffers before
selection. Creating a framebuffer does not authorize displaying it under the
currently selected constraints.

Under the CRTC modeset lock, a kernel caller sets a proposed binding with
``drm_atomic_set_constraints_for_crtc()``. The setter retains the entry, not
availability or authority. A null entry is invalid. Duplicated state retains
the accepted binding, so omission preserves selection. Only unchecked,
transaction-owned proposed state is mutable; live, detached and retiring
states cannot be rebound through the setter. Repeating selection
does not request another transition. A changed binding requires modeset
permission and marks the transaction as needing a modeset.

Core validation first adds affected plane/color state, including unchanged
active planes, then checks allocation storage and dimensions, per-plane
geometry, overlapping active-plane limits, and scalar rules after driver
checking. Geometry and scalar values come from proposed atomic state, never
current-state readback.
The provider's full-scene callback is required both during validation and
immediately before acceptance. All resources required by that callback must
already be ready. The callback must not mutate the transaction or its proposed
object states, wait for userspace or submit work. Derived-state calculation
belongs in the driver's earlier atomic checks, not in final readiness checking.

The common swap path calls constraints acceptance after predecessor waits and
driver/preparation serialization, before installing any object state. Under
the list lock it repeats availability, scene and provider checks, then
runs the infallible state-installation continuation. Failure installs nothing.
Success accepts the scene and exact backend binding together; selected-ID
readback describes accepted state, not completed presentation.

The caller stabilizes modesetting authority and affected object state.
Acceptance nests the driver's installation serialization inside modeset locks,
then preparation owner/ticket serialization, then the constraints list lock.
Provider locks needed by the constraints callback must be inside the list
lock during both ordinary validation and acceptance. The callback must not
reacquire an outer installation lock, reenter list operations or invoke
preparation operations which acquire already-held owner/ticket locks. A driver
implementing both installation and constraints callbacks must account for
their nesting; they are not independent opportunities to acquire the same
policy mutex. Native list construction alone does not establish DRM-file,
master or lease authority.

Retirement and shutdown
=======================

Accepted CRTC state retains its entry through duplication and destruction.
Providers must use that retained binding for delayed publication and keep
independent references for source work which outlives state. A mutable
device-wide current-backend pointer is not a substitute. Backend failure
after acceptance is terminal for affected work, not permission to reinterpret
the scene using another backend.

Constraints ownership does not replace source-read preparation. A released
claim can still have submitted native work in flight. Preparation guards keep
admission and native completion through atomic object cleanup; cancellation,
withdrawal and list closure do not signal that completion. Native failure
ends access when its fence signals but does not certify valid pixels.

List closure permanently excludes ordinary checking, acceptance and
listing, synchronizing with acceptance already in progress. Retained snapshots
and accepted-selection references remain valid. A fully disabled, plane-free
update may still retain the same accepted binding after closure or backend
failure. That path selects nothing new and still obeys native retirement.

Device-wide shutdown may disable several outputs in one transaction. Each
binding is checked before the single state installation. The caller's modeset
locks keep those selections unchanged through installation; the helper does
not hold several list locks at once. Closure or withdrawal does not invalidate
that operation, since it accepts no new backend work. Transactions which also
enable an output or change a binding use the full list-cohort acceptance path.

State reset restores the accepted binding even after closure. It is not
default-contract restoration or the start of a new owner interval. Full
owner-loss integration must separately close source admission, quiesce output
and restore a safe default before exposing it to a replacement client that
has not opted in. That lifecycle and client opt-in are not implemented by the
native list helpers alone.

The output separately retains the fixed default supplied at attachment.
``drm_atomic_constraints_restore_default()`` selects that default through a
blocking atomic request only after the output is disabled and plane-free.
It does not reuse an enabled scene. A changed selection must pass the default's
availability and provider checks. Failure leaves the selected binding alone;
list closure is permanent and is not undone by restoration. A repeated request
for the already selected default in an open list installs no further state.
A closed list returns ``ESTALE`` even if its default is already selected.

The caller must prevent competing modesets and replacement owners throughout
restoration and revoke the departing owner's source access beforehand. It must
not hold modeset locks across the request. The helper does not implement that
authority policy or make the default immune to later backend failure.

``drm_constraints_recover()`` performs device recovery under the same caller-owned
authority exclusion. It first disables every participating output in one prepared
request without changing the accepted backend identities. After native reads
retire, it restores each fixed default. Only after every default is restored does
it remove the other offers and clear suggestions. Independent snapshots and jobs
continue to retain their original metadata and resources.

Recovery is not an all-or-nothing modeset. If a later default fails validation,
some outputs may already have their defaults restored; all outputs remain
disabled. The caller keeps replacement owners excluded, resolves the failure
and retries. Closed lists stay closed. Neither recovery nor offer retirement
grants pixel access or supplies the caller's owner-exclusion policy.

Devices with constraints also own a recovery coordinator. Its loss notification
marks recovery pending under the native master mutex and queues work retaining
the device. The worker performs the blocking recovery outside that mutex.
Readiness checks distinguish pending work, success, recovery failure and terminal
shutdown. Failure remains visible until an explicit retry succeeds. Mode-config
cleanup stops recovery before destroying any display objects.

These coordinator operations require native master-entry paths to check readiness
before publishing replacement ownership. Merely retaining a master identity or
observing the coordinator does not authorize display changes. The coordinator
does not itself revoke source grants; loss notification follows that revocation.

Rust access and tests
=====================

``kernel::drm::constraints`` wraps descriptions, per-plane geometry, property
records, domains, typed and opaque backend entries, lists, snapshots and
encoded bytes. Native C code owns validation, identity allocation and
serialization. Rust views borrow their owning description or snapshot; entries
retain typed provider resources and their callback module. Construction and
final release require sleepable context.

During exclusive KMS setup, ``UnregisteredKmsDevice::enable_constraints()``
creates the device namespace before CRTC construction. After creating all
referenced planes and properties, ``attach_constraints()`` attaches each
output's fixed default. Both require an implemented
``KmsDriver::constraints_check()`` callback. The callback receives read-only
atomic state and opaque entry metadata during validation and final acceptance;
it must not reenter list operations or acquire modeset locks. These interfaces
use common native state lifetime and installation; backend readiness and
authority remain provider responsibilities.

``Device::constraints_output()`` borrows provider control under a registration
guard. It exposes scoped publication, snapshots, identity lookup, suggestion,
withdrawal and terminal closure, without exposing a list insertion path which
skips CRTC/plane/property membership checks. Private KMS test devices expose
the same control while their owner excludes teardown. Entry and snapshot
references can outlive that borrow without retaining the DRM device. Providers
still establish readiness and authority before publishing an entry. Closure
is permanent, not a reversible owner-interval reset or default restoration.

``Output::restore_default()`` exposes the disabled-output request described
above. It does not revoke access, establish an authority gate or disable an
active output. Its caller must provide those owner-lifetime boundaries. The
Rust CPU-provider test retains an NV12 job while disabling scanout, exercises
failed and successful default restoration, and verifies that restoration does
not reinterpret the retained job or reopen a closed list.

``CrtcStateMutator::set_constraints()`` places a retained entry in an unchecked
candidate; it does not reserve acceptance. Omission preserves the duplicated
binding. ``RawCrtcState::constraints_entry()`` borrows the exact proposed or
accepted entry, including in commit callbacks. Delayed work must retain that
binding rather than consult a later output selection. Rust tests construct
NV12 shmem framebuffers while an enabled XRGB-only entry remains selected,
then atomically switch the entry and scene and observe the target in the
commit tail. Repeated or omitted selection preserves the list generation.

``PlaneGeometry`` describes crop, fractional-source, position and scale rules
without requiring property IDs. ``RawPlane::scene_property_id()`` resolves the
remaining attached standard scalar properties without exposing raw property
storage or reading live values. Setup and runtime views return the same
identity; absent properties return ``None``. Rust providers can use those IDs
to describe stacking, blending and color rules. Native registration still
verifies geometry and property scope, type and permitted values. The Rust
property test selects an NV12 scene with BT.709 limited-range color and
restricted stacking, then rejects incompatible candidate values without
changing the accepted binding or list generation.

``OpaqueEntry::new_stateless()`` uses common DRM destruction for backends
without private per-entry resources. It avoids a permanent default retaining
its provider module solely for a metadata release callback. Device and accepted
state ownership must independently protect execution resources. Entries with
private resources retain their provider callback and module through final release.

The ``drm_constraints*`` and ``drm_atomic_constraints`` KUnit suites cover
bounded metadata, scopes, snapshots, withdrawal, closure, native state
ownership, disjoint linear/tiled framebuffer metadata and actual state swaps.
Complete-scene cases include overlay/cursor allocation, cropping, scaling,
alpha and stacking, while independent outputs repeatedly update without
sharing selection or retirement. Encoding tests cover maximum lists, zero
padding, short buffers and unaligned storage on both x86-32 and x86-64.
The ``drm_constraints_query`` suite checks bounded userspace copying, while
``drm_constraints_uapi`` exercises the actual ioctl dispatcher, sizing metadata
on ``-ENOSPC``, master admission and lease visibility.
Threaded tests hold a native read fence through target acceptance or shutdown
and verify that cleanup cannot release the predecessor backend early. The
``drm_atomic_property`` suite checks proposed-value decoding; Rust suites are
named ``rust_drm_constraints_*``.
The ``rust_drm_kms_constraints`` suite attaches a default to a Rust virtual
output with shmem framebuffers and checks validation, installation rejection,
retry and shutdown after provider failure. It does not execute a GPU backend.
The ``rust_drm_constraints_provider`` suite prepares owned mappings before
acceptance, retains the exact entry in each job, and attaches native source
accounting at commit-tail publication. Its XRGB and NV12 CPU paths produce
one-pixel results from private immutable test buffers after read admission.
Its threaded handoff test submits an old XRGB read, accepts an NV12 target,
closes the constraints list, then completes the old read under its retained
entry before target programming and readback. Kernel Rust updates use the
native prepared-request
runner, closing old source admission and retaining retirement fences without
waiting under modeset locks for unsubmitted readers.
A pending-claim test acquires the old CRTC lock while the request waits,
withdraws the checked target, then releases its read. The rebuilt request
rejects the stale target, publishes no replacement job and reopens old read
admission without changing selection.
A two-output case cycles private NV12 pixel results on one output while the
other holds an unresolved source claim. It checks independent publication,
unchanged list generations during animation, native multi-output shutdown and
disabled default restoration. A bounded reader worker releases on failure so
an isolation regression cannot strand fixture teardown.
The provider has no userspace descriptors, physical GPU or external producer.

Native atomic cohort tests accept multiple selections with one installation
and verify that a withdrawn final offer or failing final backend leaves every
selection and generation unchanged. The CastKMS renderer tests accept and
animate eight workers together through the production driver's callbacks.

Use ``kunit.filter_glob=*constraints*`` for the constraints suites and run the
atomic property suite separately or with broader DRM tests. Build modular DRM
and KMS helpers as well as built-in configurations: state initialization and
installation cross that module boundary. The standalone
``tools/testing/selftests/drm_constraints/constraints-model.py`` explores
publication orderings; it is not GPU or ioctl qualification.

Remaining integration includes coalesced DRM-event notifications, client opt-in
and normal atomic property decoding, production CastKMS backend selection and
recovery, and a real compositor consumer. Kernel-controlled provider coverage
is test evidence, not independent production-ABI demand or physical-GPU
qualification.
