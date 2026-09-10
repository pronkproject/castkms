.. SPDX-License-Identifier: GPL-2.0 OR MIT

Source-read preparation groundwork
=================================

Userspace composition of a virtual display needs to distinguish a producer
finishing an image from a reader relinquishing access to that image. Rust's
framebuffer preparation callback collects producer dependencies before ordinary
atomic acceptance. The source accounting in ``drm_atomic_prepare.c`` addresses
the other side: claims already admitted to read one source generation.

The accounting and its multi-output tickets are kernel interfaces. Atomic
helper adapters transfer preparation into accepted transactions, but do not
enable CastKMS capture by themselves. No preparation ioctl, compositor
negotiation or executor binding is provided yet.

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

The source primitive is not a complete display transaction. Output matching,
cancelable tickets and transfer into accepted commits belong to the layers
described below. Providers still need authority checks, generation publication
and preparation entry paths for blocking callers and teardown. Kernel and Rust
tests exercise the accounting without publishing source buffers or touching a
physical display.

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
keep that selection stable. Transfer into an accepted commit belongs to the
ticket and transaction adapters, not the container.

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

The guard deliberately does not provide an ``accept()`` operation. Moving it
alone does not establish display acceptance. A separate ticket and attempt
interface serializes that decision with cancellation.

Identifying retiring outputs
----------------------------

``drm_prepare_output_generation`` pairs a nonzero CRTC object ID with a retained
source generation. The provider publishes a fresh generation for every accepted
output use, including same-framebuffer updates and blank outputs. Framebuffer
identity alone cannot distinguish those uses.

``drm_prepare_outputs_create()`` copies a complete list with unique CRTC IDs and
sources in one admission domain. Its limit of 32 outputs follows the KMS CRTC
mask limit, not capture queue depth or receiver frame rate. Malformed lists
return ``-EINVAL``, oversized lists ``-E2BIG``, and mixed domains ``-EXDEV``.
Matching ignores entry order but requires the same IDs and source identities;
changed membership or generations return ``-ESTALE``. An empty list matches
only an empty observation. It is not a wildcard.

The list retains accounting objects, not pixel storage, authority or admission
holds. ``drm_prepare_outputs_hold()`` derives a retirement set from precisely
those sources. Ticket construction performs both operations. Rust callers use
``OutputGeneration`` and pass the complete borrowed slice to ``Ticket::new()``;
the ticket takes independent references before construction returns.

Reserving a cancelable request
-----------------------------

An internal ``drm_prepare_ticket`` captures output generations and retains their
source set independently of any file. It represents a cancelable request, not a
grant of pixel access. ``drm_prepare_ticket_reserve()`` first
collects a retirement guard, then reserves the ticket for one attempt. Pending
source claims reject reservation without consuming the request. A competing
attempt receives ``-EBUSY``. Allocation and completion collection happen outside
the ticket mutex, with availability checked again before publication.

An attempt retains both its ticket and its own guard. Destroying an unsuccessful
attempt allows another reservation if the ticket remains live. Canceling the
ticket prevents installation and drops the ticket's source-set reference, but
does not destroy an active attempt's guard. That distinction prevents
cancellation from reopening admission while an operation still owns the old
sources. Reference release is not cancellation; a file or authority
adapter must call cancellation explicitly at its specified lifetime boundary.

``drm_prepare_attempt_commit()`` runs a kernel installation callback under the
same mutex used by cancellation. It compares the complete observed output list
with the ticket before calling the installer. The callback must reject invalid
authority before changing anything, or install the complete transaction without
another fallible step. It must not allocate, wait or reenter preparation. On
success the call transfers the preassembled guard to the caller and consumes
the ticket once. Cancellation after that point cannot revoke the accepted
guard. The attempt still needs destruction, separately from its returned guard.

Display and provider locks that stabilize the selected generations precede the
ticket mutex. No source lock is nested inside it: fence collection and source
ownership destruction occur outside that mutex. The ticket does not acquire
display locks for its caller, derive current generations or authenticate a
requester.

Rust ownership follows the same distinction. A ``Ticket`` is shared through
reference counting and must be canceled explicitly when its authority ends.
``Ticket::reserve()`` returns a unique ``Attempt`` that retains the reservation
even if every external ticket reference is dropped. Moving that owner is
allowed; copying it or constructing it from a raw pointer is not a public
operation. Dropping an unaccepted attempt releases its reservation and permits
a still-live ticket to retry.

The Rust interface does not expose an arbitrary installation callback. Owning
an attempt is not evidence that a caller has validated the retiring outputs.
The reservation alone does not establish successful display installation. Runtime
tests exercise cancellation and release, while compiler fixtures reject
duplication and construction outside the reservation path.

A caller that owns a retirement set may wait interruptibly with
``drm_prepare_retirement_set_wait()`` or Rust's
``RetirementSet::wait_prepared()``. Claim release and abandonment notify a
wait queue shared by the source domain. The wait checks every member: one
abandoned claim ends it with an error even while another member remains
pending. Rust returns a retained prepared set, so dropping the original set
does not invalidate the result.

Readiness may precede completion of every submitted GPU read. An interrupted
wait also leaves the caller's admission holds intact for retry or release.
Callers must not hold display or provider locks needed by the claim owners
while waiting. Waiting on a separately owned set is not a ticket cancellation
operation; canceling a ticket does not withdraw that independent set ownership.

For a request whose lifetime follows a ticket, use ``drm_prepare_ticket_wait()``
or Rust's ``Ticket::wait_ready()`` instead. Cancellation wakes a pending caller
with a cancellation error even if an admitted claim remains unresolved.
Consumption reports that the ticket has already been used. A successful wait
only observes readiness: another caller may reserve or consume the ticket, so
reservation and installation must still recheck its state.

Ticket waits retain a temporary set reference until they return. Cancellation
does not release that caller's holds underneath it. Notifications are shared by
the source domain, but cancellation belongs to one ticket; a peer ticket must
recheck its own state rather than interpreting any wakeup as cancellation.

Observing a ticket through a file
--------------------------------

A ticket retains its notification domain independently of source admission.
Its borrowed wait queue therefore remains valid after cancellation releases
the source set, for as long as the observer retains the ticket. An empty ticket
has its own notification domain. Observers register before querying readiness
and unregister before releasing their final ticket reference.

``drm_prepare_ticket_file_create()`` places that observation behind an anonymous,
poll-only file. Pending preparation has no poll events; readiness reports
readable; consumption reports hangup; cancellation or source failure reports
error and hangup. Readability is an observation that preparation is ready, not
an invitation to read a byte stream. It neither reserves the ticket nor proves
that GPU reads have finished.

The file has no read, write, mmap or ioctl operations. Checked kernel lookup
through ``drm_prepare_ticket_file_get_ticket()`` returns an owned ticket
reference, rejecting other file types. The Rust transport module exposes the
same ownership through ``Ticket::create_file()`` and ``Ticket::from_file()``;
the ticket implementation itself does not depend on file operations.

Final file release cancels the ticket, including when an unpublished file is
discarded. Duplicated file references share that lifetime. Closing one descriptor
need not release the file while duplicates or active operations retain it, and
file release may be deferred. Prompt revocation must use explicit cancellation.
Creating a separate file for the same ticket creates another cancellation
handle, not another independent request. Retaining a kernel ticket reference
does not prevent file-driven cancellation; an accepted retirement guard remains
independently owned.

The constructor installs no descriptor and does not authenticate display access.
A future issuer must validate the request, reserve descriptors with close-on-exec
and complete fallible setup before publication. The native and Rust file tests
exercise polling and reference release without exposing a preparation-creation
ioctl or enabling delegated source access.

Where the helper installs display state
--------------------------------------

``drm_atomic_helper_swap_state()`` has two phases. When requested, it first
waits for preceding CRTC, connector and plane commits to finish programming
hardware. The private ``wait_for_previous_hw_done()`` helper performs those
interruptible waits in their existing order. A failed wait leaves every object
state pointer unchanged. Only after success does the swap routine start
installing the new pointers, under the caller's modeset locks.

``drm_atomic_helper_swap_state_prepared()`` runs those same waits before entering
the ticket's serialized decision. Its installation callback uses the same
private installer as ordinary swaps. Cancellation rejects the prepared swap
before the first pointer changes; successful installation transfers the guard
and consumes the ticket without an interval of reopened admission.

The caller must supply every retiring output generation and keep the list
stable under its display and provider locks throughout the call. Authority invalidation must
either cancel the ticket or participate in that same caller-owned locking. The
caller must not hold a lock needed for predecessor completion across the waits.
The helper does not infer source generations from framebuffer pointers. Its returned
guard also does not itself delay old framebuffer cleanup: the caller must join
the retained native completion to its normal retirement path before releasing
old source use.

Transactions without preparation retain ordinary helper behavior. The separate
prepared-swap entry point leaves guard ownership and completion waits to its
caller. The entry point checks the supplied generations, but does not derive
them from the expanded atomic update or authenticate the submitting file.
Those provider and issuer adapters are required before enabling delegated
capture; no pixel-export interface is supplied here.

Native tests call the real swap helper with isolated state records. They
interrupt each predecessor class and check controllers, connectors, planes,
color operations and driver-private objects together. The published pointers,
the records selected for cleanup and the transaction backpointers must all
remain unchanged. A successful retry must transfer each of those records to
its installed position. Color operations and private objects have no separate
predecessor wait in the helper; they still belong to the complete group that
must remain unpublished when another member's wait is interrupted.

Completed and absent predecessors and the no-stall path exercise successful
installation of the same group. These tests do not run driver callbacks,
validate a display configuration or establish locking for a future ticket
interface. They also do not qualify commit-list or event ownership: the new
controller state has no commit record in these fixtures.

Prepared-swap tests use those same object records to exercise interruption
followed by retry, cancellation before installation, and accepted ownership
surviving ticket release. The ticket tests separately race cancellation against
a synthetic installation callback. Neither test fixture supplies real display
locks or proves the caller's generation and authority checks.

Preparation owned by the transaction
-----------------------------------

A driver using the shared atomic commit helpers can attach a ticket with
``drm_atomic_commit_prepare()`` instead of retaining a separate attempt beside
the transaction. A mandatory observer supplies the complete output-generation
list at installation, under the caller's display and provider locks. The call
reserves one attempt and allocates its owner before
installation. It may fail without changing the transaction. Attaching a second
reservation is rejected; a caller changing the retiring outputs must clear the
old transaction and construct a new one.

The ordinary swap helper consumes attached preparation at the same serialized
installation decision described above. Until installation succeeds, cancellation
still prevents acceptance. An interrupted predecessor wait leaves the same
reservation available for retry. Successful installation retains the accepted
guard in the transaction, independently of ticket cancellation or release.

The shared commit dependency wait also waits for the submitted native readers.
That wait occurs before hardware programming and notification that the old
display use has retired. It is not a wait for userspace to submit more work.
An error completion ends native access without establishing that the captured
pixels are valid. Driver-specific commit paths must perform the same reader
wait before reporting retirement or releasing old source use.

Atomic core cleanup waits for accepted readers before calling the driver's
object-clear callback, then releases preparation only after that callback has
finished. Admission therefore remains held while old state is destroyed, even
if the normal commit-tail wait was not reached. Clearing an unaccepted update
instead abandons its reservation without waiting for readers or canceling the
ticket. A still-live ticket may then be reserved for a rebuilt transaction.

The async plane-update shortcut is rejected when preparation is attached: that
path does not use the ordinary installation decision. Nonblocking atomic
commits still use that decision and are not the same operation as async plane
updates.

These operations manage ownership, not display policy. The caller must still
validate every retiring source generation and its authority, and keep that
selection stable through installation. No ioctl or source-export facility is
enabled merely by adding a transaction owner. The native tests use a custom
object-clear callback and submitted test fences to check lifetime and wait
ordering; they do not qualify a physical GPU or a complete capture provider.
