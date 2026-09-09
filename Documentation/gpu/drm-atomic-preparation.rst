.. SPDX-License-Identifier: GPL-2.0 OR MIT

Source-read preparation groundwork
=================================

Userspace composition of a virtual display needs to distinguish a producer
finishing an image from a reader relinquishing access to that image. Rust's
framebuffer preparation callback collects producer dependencies before ordinary
atomic acceptance. The source accounting in ``drm_atomic_prepare.c`` addresses
the other side: claims already admitted to read one source generation.

The accounting is a kernel-only primitive. It neither changes atomic commit
semantics nor enables CastKMS capture. No preparation ioctl, multi-output
ticket, compositor negotiation or executor binding is provided yet.

Admission, release and completion
--------------------------------

A provider allocates one source generation with an explicit capacity. Before
claiming a read, it must establish pixel authority, retain the source storage
and have independent storage available for the result. Waiting for an encoder
or exported destination to become reusable must remain source-unbound.
The primitive cannot inspect or enforce a native GPU dependency graph.

Sealing permanently closes admission and cannot be undone. An admission hold
instead blocks new claims until its final reference is released, provided no
other hold or permanent seal remains. Releasing one of several overlapping
holds never reopens admission. Neither operation pauses GPU execution or
freezes pixel contents; both serialize with admission of new read claims.
A generation with admission closed becomes ready only after every admitted
claim has been released. Release consumes a claim and promises no more access
under it. It either reports ended synchronous access or supplies an
already-materialized native fence covering all submitted reads. Future
userspace submission is not a fence.

Readiness does not wait for those native fences to signal. It establishes a
fixed completion set with no unresolved userspace handoff. The provider can
obtain an owned native completion fence from that set. A failed native fence
still establishes ended access; it does not establish valid captured pixels.
The provider must keep its source storage and normal KMS retirement obligations
independently of the accounting allocation.

Capacity includes unresolved claims and released reads with pending native
completion. While admission remains open, completed readers are reclaimed when
another claim is attempted. The limit is chosen by the provider, not derived
from receiver frame rate or a universal queue depth.

Ownership and failure
---------------------

Each unresolved claim retains its source. Released native records belong to
the source itself, without retaining a reference back to it. Native fence
destruction and completion merging occur outside the accounting mutex.
Dropping the last external source reference does not fabricate claim release.

The Rust wrapper's ``ReadClaim`` is non-cloneable and release consumes it.
The name describes admission ownership, not proof that a read has occurred.
A ``PreparedSource`` retains either permanent closure or its own
``AdmissionHold`` reference; dropping the original hold cannot invalidate
that proof. Source-level readiness requires permanent closure, so an unrelated
hold owner cannot give another caller a preparation proof that disappears
when the hold is released.
Dropping an unreleased claim marks terminal service failure. Such a generation
never yields a prepared source, even after other claims are released. That
failure does not assert that unknown GPU work stopped; executor loss still
requires best-effort supervision and resource cleanup by the provider.

Reopening admission preserves unresolved claims and submitted native readers.
Releasing an admission hold does not cancel access already admitted. Later
preparation must still account for those readers before establishing readiness
and native completion.

The primitive must not be installed as a complete atomic preparation ticket:
validated multi-output scope, gap-free transfer to accepted commits, blocking
internal callers and teardown integration remain separate work. Kernel and Rust tests
exercise the primitive without publishing source buffers or touching a physical
display.

Holding several sources together
-------------------------------

A display update may replace several images. Closing admission one image at a
time would let another caller observe only part of that update's sources held.
A provider can instead create an admission domain and place related sources in
it with ``drm_prepare_source_create_in()``. The domain supplies one lock for
their accounting decisions. Independent providers use independent domains;
there is no global preparation lock. The convenience source constructor still
creates a private domain for a single independent source.

``drm_prepare_retirement_set_create()`` accepts a borrowed array of retained
sources in one domain. It copies the array, coalesces repeated source identities
and allocates its hold storage before changing admission. Under the domain lock
it checks every member before installing any hold. A failed member therefore
leaves no partially held set. Sources in different domains are rejected rather
than acquired one domain at a time. An empty collection is valid.

The returned set owns every hold until its last reference is released.
Overlapping sets retain independent holds, so releasing one set cannot reopen
a source still held by another. Releasing a set does not resolve existing read
claims, cancel GPU access or undo a permanent seal. Each hold retains its source,
and each source retains its domain, independently of the provider's original
references. Source storage and pixel authorization still belong to the provider.

Rust callers use ``Domain`` and ``Source::new_in()`` to construct related sources.
``RetirementSet::new()`` borrows a slice of retained sources and returns its own
reference-counted set. The input array and the original domain owner may then
be dropped without releasing the set's holds. No raw pointer or manual native
reference transfer is part of the Rust interface. These wrappers live in the
domain and set submodules of ``drm::preparation``.

The set is an internal ownership container, not a validated display transaction.
Its caller must determine which generations the update actually retires and
keep that selection stable. Transfer into an accepted commit is not provided
by the container yet.

Preparing the complete set
-------------------------

Set readiness means every admitted claim has either finished reading on the
CPU or supplied completion for an already submitted native read. An abandoned
claim reports a terminal error, even if another member still has a pending
claim. Readiness does not wait for submitted GPU work to finish. Once a member
is ready, the set's retained hold prevents new claims from changing that result.

``drm_prepare_retirement_set_completion()`` collects and merges the members'
native fences only after readiness. Failure leaves the caller's output
unchanged. Empty sets and synchronous reads need no fence. A returned fence
owns its dependencies independently of the sources and the set, but does not
own admission holds. Keeping only the fence would therefore allow new readers
after the last set owner releases its holds.

Rust exposes that distinction through ``RetirementSet::prepared()``. Pending
claims produce no prepared owner; success returns ``PreparedRetirement``, which
retains the complete set and provides access to native completion. Its private
fields prevent construction without the readiness check. The owner must remain
alive until a later operation takes responsibility for keeping admission closed.
Neither successful preparation nor fence completion proves pixel validity,
capture authority or acceptance of a display transaction.

Assembling ownership before acceptance
-------------------------------------

An acceptance path must not install new display state and then discover that
collecting completion needs an allocation that can fail. A retirement guard
assembles that ownership in advance. ``drm_prepare_retirement_guard_create()``
borrows a ready set, collects its native completion and takes its own set
reference. Failure leaves the caller's preparation intact for a retry.

The guard is a unique owner. Its completion getter borrows the same retained
fence without allocating or visiting source records again, even after the
native reads finish. Moving the guard into another owner does not release any
admission holds in between. Destroying it releases its references without
waiting for native work or claiming that such work was cancelled. A caller
that needs pixel storage must retain that storage separately.

Rust provides ``RetirementGuard::new(&prepared)``. The result cannot be copied
or cloned, and a borrowed completion reference cannot outlive the guard. A
caller can explicitly retain the fence, but that separate reference does not
keep admission closed. Abandoning a prospective commit's guard leaves the
original preparation owner available for another attempt.

The guard deliberately does not provide an ``accept()`` operation. Acceptance
must serialize display-state installation with scope validation and ticket
consumption. The guard supplies the ownership that can cross that boundary;
moving it alone does not establish that the boundary has been crossed.

Where the helper installs display state
--------------------------------------

``drm_atomic_helper_swap_state()`` has two phases. When requested, it first
waits for preceding CRTC, connector and plane commits to finish programming
hardware. The private ``wait_for_previous_hw_done()`` helper performs those
interruptible waits in their existing order. A failed wait leaves every object
state pointer unchanged. Only after success does the swap routine start
installing the new pointers, under the caller's modeset locks.

A future preparation acceptance hook belongs after those waits and before
the first installation. It must serialize ticket cancellation and scope
validation with the complete installation, rather than consuming a ticket
before calling the swap helper. The current code only separates the wait
phase; it does not add that hook or attach retirement guards to transactions.

Native tests call the real swap helper with isolated state records. They
interrupt each predecessor class, check that all pointers remain unchanged,
then retry successfully. Completed and absent predecessors and the no-stall
path are covered too. These tests do not run driver callbacks, validate a
display configuration or establish locking for a future ticket interface.
