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

Only one independent output is admitted per transaction involving constraints.
Such a transaction must contain exactly one CRTC, and asynchronous plane
updates are rejected. Drivers which do not attach constraints keep their
ordinary atomic behavior.

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
records per description, and at most 64 entries per output catalog. Providers
choose their retained-entry quota and may choose a smaller catalog limit.
These are kernel prototype bounds, not allocated wire-ABI constants.

Catalogs and snapshots
======================

A catalog begins with a nonzero accepted selection. Adding an entry does not
select it or change current buffer validity. A suggestion is advisory; zero
clears it. Withdrawing an offer excludes new selection but cannot undo an
accepted scene. A withdrawn, unselected listing can be forgotten while
independent references continue retaining the entry.

``drm_constraints_catalog_snapshot()`` returns a bounded immutable list with
generation, selected ID, suggested ID and availability metadata. A nonzero
expected generation must match or the call returns ``-ESTALE``. Snapshots
retain every listed entry independently of catalog changes or closure.
Generation changes concern entries, availability, selection and suggestions,
not ordinary repeated frames. No-op metadata operations preserve it. Identity
and generation exhaustion return ``-EOVERFLOW`` instead of wrapping.

Listing positions are not identities. Neither obtaining a snapshot nor
checking one of its entries reserves later acceptance. Callers must tolerate
withdrawal and readiness loss before the final decision.

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
the accepted binding, so omission preserves selection. Repeating selection
does not request another transition. A changed binding requires modeset
permission and marks the transaction as needing a modeset.

Core validation first adds affected plane/color state, including unchanged
active planes, then checks allocation and scalar rules after driver checking.
Scalar values come from proposed atomic state, never current-state readback.
The provider's full-scene callback is required both during validation and
immediately before acceptance. All resources required by that callback must
already be ready; it must not wait for userspace or submit work.

The common swap path calls constraints acceptance after predecessor waits and
driver/preparation serialization, before installing any object state. Under
the catalog lock it repeats availability, scene and provider checks, then
runs the infallible state-installation continuation. Failure installs nothing.
Success accepts the scene and exact backend binding together; selected-ID
readback describes accepted state, not completed presentation.

The caller stabilizes modesetting authority and affected object state. Lock
ordering is caller authority/modeset locks, then catalog serialization, then
provider locks needed by the callback. Callbacks must not reenter catalog
operations or acquire caller locks again. Native catalog construction alone
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
withdrawal and catalog closure do not signal that completion. Native failure
ends access when its fence signals but does not certify valid pixels.

Catalog closure permanently excludes ordinary checking, acceptance and
listing, synchronizing with acceptance already in progress. Retained snapshots
and accepted-selection references remain valid. A fully disabled, plane-free
update may still retain the same accepted binding after closure or backend
failure. That path selects nothing new and still obeys native retirement.

State reset restores the accepted binding even after closure. It is not
default-contract restoration or the start of a new owner interval. Full
owner-loss integration must separately close source admission, quiesce output
and restore a safe default before exposing it to a replacement client that
has not opted in. That lifecycle and client opt-in are not implemented by the
native catalog helpers alone.

Rust access and tests
=====================

``kernel::drm::constraints`` wraps descriptions, property records, domains,
typed backend entries, catalogs and snapshots. Native C code owns validation,
identity allocation and serialization. Rust views borrow their owning
description or snapshot; entries retain typed provider resources and their
callback module. Construction and final release require sleepable context.
These metadata wrappers do not provide Rust KMS attachment or installation.

The ``drm_constraints*`` and ``drm_atomic_constraints`` KUnit suites cover
bounded metadata, scopes, snapshots, withdrawal, closure, native state
ownership, disjoint linear/tiled framebuffer metadata and actual state swaps.
Threaded tests hold a native read fence through target acceptance or shutdown
and verify that cleanup cannot release the predecessor backend early. The
``drm_atomic_property`` suite checks proposed-value decoding; Rust suites are
named ``rust_drm_constraints_*``.

Use ``kunit.filter_glob=*constraints*`` for the constraints suites and run the
atomic property suite separately or with broader DRM tests. Build modular DRM
and KMS helpers as well as built-in configurations: state initialization and
installation cross that module boundary. The standalone
``tools/testing/selftests/drm_constraints/constraints-model.py`` explores
publication orderings; it is not GPU or ioctl qualification.

Remaining integration includes a bounded versioned listing/copyout contract,
coalesced DRM-event notifications, client opt-in and normal atomic property
decoding, owner-interval/default restoration, and a real provider/compositor
consumer. No native test establishes those userspace or physical-GPU results.
