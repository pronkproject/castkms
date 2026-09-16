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

The implementation is a kernel-only prototype. It does not allocate a client
capability, listing ioctl, selection property or notification event. No
CastKMS provider is attached to these helpers. The native test provider uses
framebuffer metadata, not GPU allocation or PRIME import. Those boundaries
must not be inferred from successful native tests.

Selecting constraints or updating an enabled scene admits only one independent
output per transaction involving constraints. A full disable may include
multiple CRTCs when every CRTC in the transaction is disabled, plane-free and
retains its accepted binding. Asynchronous plane updates are rejected. Drivers
which do not attach constraints keep their ordinary atomic behavior.

Descriptions and identities
===========================

``drm_constraints_description_create()`` copies immutable allocation and
scalar property records. Inclusive nonzero output and framebuffer dimension
bounds may describe exact sizes by making each minimum equal its maximum.
Framebuffer bounds concern allocation, not the fractional source rectangle.
Each allocation record names an existing plane and a standard DRM
format/modifier pair. Tiled layouts are not restricted to software-compositor
capabilities.

Scalar rules use the native DRM range, signed-range, enum or bitmask type.
Signed bounds retain DRM's two's-complement unsigned representation. Enum
rules describe permitted values 0 through 63 by their mask bits; bitmask
rules describe permitted bits. A zero bitmask allows only zero; an enum mask
must be nonempty. Unknown rule types, duplicate object/property pairs and
malformed bounds are rejected, not interpreted as unrestricted support.

Output attachment and publication validate object membership, property
attachment, property type and bounds within existing discovery. Supported
scalar scene properties cover plane geometry, alpha, blending, rotation,
stacking, YUV encoding/range, scaling filters and cursor hotspots, together
with CRTC VRR, background color, scaling filter and sharpness. Read-only and
request-only properties, object IDs, blob contents and driver-private
properties are not scalar rules. Rules constrain enabled outputs and the
planes used by their scenes, not unused objects.

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

The current native bounds are 256 format records and 64 scalar property
records per description, and at most 64 entries per output list. Providers
choose their retained-entry quota and may choose a smaller list limit.
These are kernel prototype bounds, not allocated wire-ABI constants.

Lists and snapshots
======================

A list begins with a nonzero accepted selection. Adding an entry does not
select it or change current buffer validity. A suggestion is advisory; zero
clears it. Withdrawing an offer excludes new selection but cannot undo an
accepted scene. A withdrawn, unselected listing can be forgotten while
independent references continue retaining the entry.

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
output-dimension, per-plane format/modifier and scalar-property records.
Every offset is relative to the start of the complete snapshot. Per-plane
format records are alternatives; scalar rules apply together. Unknown
required records make an entry unusable, not unrestricted.

The encoding is bounded to one MiB, including a maximum-size native list.
All padding and reserved output fields are zero. A null buffer with zero
capacity discovers the required size. An undersized buffer returns
``-ENOSPC`` and the required size without modifying any payload. Other errors
leave the size output unchanged. Success writes exactly the required bytes,
including when the kernel buffer is unaligned.

The snapshot retains its original generation, identities and availability
even after the list changes or closes. The serialized bytes themselves
retain no resources and grant no authority. These are kernel-only layout
definitions, not an installed UAPI. The encoder does not implement a userspace
ioctl, request validation, failure-copyout semantics or event delivery.

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
active planes, then checks allocation and scalar rules after driver checking.
Scalar values come from proposed atomic state, never current-state readback.
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

The caller stabilizes modesetting authority and affected object state. Lock
ordering is caller authority/modeset locks, then list serialization, then
provider locks needed by the callback. Callbacks must not reenter list
operations or acquire caller locks again. Native list construction alone
does not establish DRM-file, master or lease authority.

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
that operation, since it accepts no new backend work. Enabling an output or
changing any binding in the same transaction remains unsupported.

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
for the already selected default installs no further state.

The caller must prevent competing modesets and replacement owners throughout
restoration and revoke the departing owner's source access beforehand. It must
not hold modeset locks across the request. The helper does not implement that
authority policy or make the default immune to later backend failure.

Rust access and tests
=====================

``kernel::drm::constraints`` wraps descriptions, property records, domains,
typed and opaque backend entries, lists, snapshots and encoded bytes. Native C code owns
validation, identity allocation and serialization. Rust views borrow their owning
description or snapshot; entries retain typed provider resources and their
callback module. Construction and final release require sleepable context.

During exclusive KMS setup, ``UnregisteredKmsDevice::enable_constraints()``
creates the device namespace before CRTC construction. After creating all
referenced planes and properties, ``attach_constraints()`` attaches each
output's fixed default. Both require an implemented
``KmsDriver::constraints_check()`` callback. The callback receives read-only
atomic state and opaque entry metadata during validation and final acceptance;
it must not reenter list operations or acquire modeset locks. These interfaces
use common native state lifetime and installation without publishing UAPI.

``Device::constraints_output()`` borrows provider control under a registration
guard. It exposes scoped publication, snapshots, identity lookup, suggestion,
withdrawal and terminal closure, without exposing a list insertion path which
skips CRTC/plane/property membership checks. Private KMS test devices expose
the same control while their owner excludes teardown. Entry and snapshot
references can outlive that borrow without retaining the DRM device. Providers
still establish readiness and authority before publishing an entry. Closure
is permanent, not a reversible owner-interval reset or default restoration.

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
Threaded tests hold a native read fence through target acceptance or shutdown
and verify that cleanup cannot release the predecessor backend early. The
``drm_atomic_property`` suite checks proposed-value decoding; Rust suites are
named ``rust_drm_constraints_*``.
The ``rust_drm_kms_constraints`` suite attaches a default to a Rust virtual
output with shmem framebuffers and checks validation, installation rejection,
retry and shutdown after provider failure. It does not execute a GPU backend.

Use ``kunit.filter_glob=*constraints*`` for the constraints suites and run the
atomic property suite separately or with broader DRM tests. Build modular DRM
and KMS helpers as well as built-in configurations: state initialization and
installation cross that module boundary. The standalone
``tools/testing/selftests/drm_constraints/constraints-model.py`` explores
publication orderings; it is not GPU or ioctl qualification.

Remaining integration includes userspace listing validation and error copyout,
coalesced DRM-event notifications, client opt-in and normal atomic property
decoding, owner-interval/default restoration, and a real provider/compositor
consumer. No native test establishes those userspace or physical-GPU results.
