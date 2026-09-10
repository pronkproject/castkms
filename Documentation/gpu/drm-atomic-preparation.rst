.. SPDX-License-Identifier: GPL-2.0 OR MIT

Source-read preparation groundwork
=================================

Userspace composition of a virtual display needs to distinguish a producer
finishing an image from a reader relinquishing access to that image. Rust's
framebuffer preparation callback collects producer dependencies before ordinary
atomic acceptance. The source accounting in ``drm_atomic_prepare.c`` addresses
the other side: claims already admitted to read one source generation.

The accounting and its multi-output tickets have kernel interfaces and an
experimental userspace adapter. Participating virtual drivers can issue tickets
for their current display state and accept them with atomic updates. Those
operations do not enable CastKMS capture by themselves: executor binding and
pixel access remain separate work.

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
the source set, for as long as the observer retains the ticket.
Observers register before querying readiness
and unregister before releasing their final ticket reference.

``drm_prepare_ticket_file_create()`` places that observation behind an anonymous,
file. Pending preparation has no poll events; readiness reports
readable; consumption reports hangup; cancellation or source failure reports
error and hangup. Readability is an observation that preparation is ready, not
an invitation to read a byte stream. It neither reserves the ticket nor proves
that GPU reads have finished.

The file has no read, write or mmap operations. ``DRM_IOCTL_PREPARE_QUERY``
reports pending, ready, consumed, canceled or failed status without consuming
the ticket. Checked kernel lookup
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
Its issuer must validate the request, reserve descriptors with close-on-exec
and complete fallible setup before publication. The native and Rust file tests
exercise polling and reference release independently of the creation ioctl
described below. Neither interface enables delegated source access.

Issuing tickets for atomic updates
---------------------------------

A participating device advertises ``DRM_CAP_ATOMIC_PREPARATION``. A client
first enables ordinary atomic modesetting, then enables
``DRM_CLIENT_CAP_ATOMIC_PREPARATION`` on the same DRM file. The negotiated
interface requires a ticket for each non-test atomic update, including blocking
updates. Its capability and ioctl numbers are experimental, not an assigned
upstream ABI.

``DRM_IOCTL_MODE_PREPARE_REPLACE`` accepts one to 32 unique CRTC IDs belonging
to that file's modesetting authority. It captures their current accepted
generations under the display locks and prevents new read admission to them.
The ioctl does not wait for outstanding claims to be released. It returns a
close-on-exec descriptor as its positive return value; the input structure has
no output fields. Descriptor installation follows all fallible setup, so a
failed result copy cannot leave an undisclosed descriptor in the caller.

The client waits for that descriptor outside the atomic ioctl and queries its
status after a wakeup. READY describes submission preparation, not GPU
completion. Every covered CRTC in the replacement request carries the same
descriptor through ``PREPARE_FD``. The property is an input for that request,
not persistent display state; reading it returns -1. Descriptor numbers are
never ticket identities, and the atomic transaction retains the ticket rather
than the descriptor or its file.

Acceptance checks the issuing authority and the complete CRTC generation set
after driver validation has expanded the atomic update. An incomplete or stale
set returns ``ESTALE`` without installing new state. The client must discard
the old ticket and prepare the intended output set again. Pending preparation
returns ``EAGAIN``; a competing ordinary update may still produce ``EBUSY``.
Neither error means that display state was accepted. Closing or revoking a
ticket cancels an unaccepted request, while an accepted transaction retains
its own retirement obligations.

TEST_ONLY does not require a ticket and never reserves or consumes one. A
supplied descriptor is checked for type and issuer, but TEST_ONLY does not
promise that its generation set will match a subsequent real commit. Async
plane updates are not supported by the participating display accounting.

The kernel equivalent separates selection from transport.
``drm_atomic_prepare_crtcs()`` captures locked CRTC generations;
``drm_atomic_prepare_submission_init()`` and ``_set()`` collect an issuer and
ticket for the submitted CRTCs. ``_attach()`` gives the ordinary transaction
the preparation obligation. These operations need no userspace descriptor.
Drivers opt in before constructing CRTCs, use the shared state lifetime and
installation helpers, and obtain a new generation for each accepted CRTC use,
including an unchanged framebuffer or blank output.

VKMS exposes the experiment with ``vkms.enable_preparation=1``; the option is
off by default. The Rust CastKMS device enables the same accounting through its
unregistered-device wrapper. Neither provider admits external pixel readers
yet. Kernel shutdown uses the request entry described below. Legacy updates,
other internal callers, suspend and file-close teardown still need preparation
integration before delegated reading can be enabled. Testing explicit blocking
tickets does not establish those paths.

Rebuilding a blocking kernel request
-----------------------------------

An internal caller may describe its operation to ``drm_atomic_commit_request()``
instead of assembling an atomic state that must survive a wait. The helper
owns its modeset lock context and allocates a fresh atomic state for each
attempt. Its callback applies the requested operation to that state using the
ordinary atomic getters. The callback does not install anything itself.

After checking has added every affected output, the helper captures a kernel
ticket for those outputs. If an admitted reader has not relinquished its claim,
the helper destroys the attempted atomic state, drops the modeset locks and
waits. The ticket retains the admission holds, but no borrowed display state
survives that interval. After readiness, the callback runs again against the
current display state. A replacement ticket acquires its holds before the
previous ticket is released, so unchanged generations do not reopen admission
during rebuilding. A concurrent display update instead causes the rebuilt
request to prepare the newly current generations.

Checking, ticket reservation and the driver's blocking commit then use the
same locked state. Installation still validates the complete output set and
transfers native completion into the accepted transaction. Preparation failure
or interruption returns an error without installing the request. An ordinary
lock deadlock follows the modeset backoff protocol and also rebuilds the state.

The caller must enter without modeset locks or other locks needed by readers.
It owns the operation's inputs and keeps the device and callback alive until
return. For example, a framebuffer reference or producer fence must remain a
reference, not a handle or descriptor looked up again after waiting. Any
authority needed by the operation must be revalidated when rebuilding.
These requirements are part of the kernel callback contract; the helper does
not freeze userspace memory or supply an adapter for ordinary atomic ioctls.

``drm_atomic_helper_shutdown()`` uses that entry on participating devices.
Its operation is to disable every output, which it reconstructs from current
state after a wait. The caller must stop new display producers and retain the
device resources until shutdown finishes. Devices without preparation retain
the ordinary shutdown path. Suspend is separate because saving state for resume
must be coordinated with the eventual disabling transaction.

Kernel tests use an outstanding read claim and a second thread which needs the
modeset lock before releasing it. They check rebuilding, intervening generation
changes, rejected requests and shutdown without depending on a userspace
executor. The test driver installs state through the real swap helper; it does
not program display hardware or exercise native GPU execution.

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
stable under its display and provider locks throughout the call. Authority
invalidation must either cancel the ticket or participate in that same
caller-owned locking. The
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

Following modesetting authority
------------------------------

``drm_file_prepare_owner()`` returns a retained issuer identity for the current
modesetting master file, including a lease master. Duplicated descriptors refer
to the same file and issuer. A separate file associated with that master is not
the issuing master file and cannot borrow its issuer identity. The reference
preserves identity, not permission to use any particular display object.

``drm_prepare_ticket_create_owned()`` binds a request to that continuous issuer
lifetime. An issuer admits at most 256 retained tickets, including terminal
tickets whose references have not been released. That allocation bound is not
a capture queue limit. Reservation checks the issuer identity, and acceptance
serializes with its revocation before entering the ticket's own decision.

Dropping master revokes the issuer and its lease descendants. Revoking a lease
revokes its subtree without canceling sibling issuers. Final release of the
issuing file cancels preparation before framebuffer and other display-state
teardown. Closing an associated file or one of several references to the issuing
file does not trigger that boundary. Master reacquisition obtains a fresh
issuer; retaining an old owner or ticket never revives it. An already accepted
retirement guard remains responsible for its source holds and native readers.

Issuer lookup takes ``master_mutex`` followed by ``mode_config.idr_mutex``.
Callers must not hold display locks during lookup. Lease-tree cancellation
takes the owner and ticket locks under ``idr_mutex`` and never waits for future
userspace submission or native GPU completion. Owners do not retain DRM files,
so retained ticket descriptors cannot keep an issuing file alive in a cycle.

These hooks are not a preparation ioctl. A provider must still authorize every
selected display object, publish current generations, and stabilize the complete
retiring list through installation. The existing atomic attachment helper takes
unowned kernel tickets; accepting file-issued tickets needs a separate adapter
that checks the issuer. Device removal and blocking internal entry paths also
need their provider integration before delegated execution is enabled.

KUnit tests use controlled master trees and real master drop/reacquisition,
file-release and lease-revocation paths. They check associated-file rejection,
duplicate-reference lifetime and descendant cancellation. They do not exercise
lease creation through an ioctl, unplug or a physical display.
