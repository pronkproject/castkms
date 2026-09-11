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
interface requires a ticket for nonblocking atomic updates. Blocking updates
without any ``PREPARE_FD`` assignment prepare internally, whether or not the
client negotiated explicit tickets. A supplied ticket keeps the explicit
protocol described below. Capability and ioctl numbers are experimental, not
an assigned upstream ABI.

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
A required but absent ticket returns ``EINVAL`` rather than a readiness error.
None of those errors means that display state was accepted. Closing or revoking a
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
yet. Kernel shutdown, suspend, framebuffer removal and kernel display clients
use the request entry described below. The legacy SETCRTC ioctl uses a resolved
request callback on participating providers. Other legacy userspace updates and
internal callers still need preparation integration before delegated reading
can be enabled. Neither explicit nor implicit atomic commits establish those
remaining legacy paths.

Blocking atomic updates from userspace
-------------------------------------

On a participating provider, a blocking atomic ioctl without any ``PREPARE_FD``
assignment uses the retained request adapter. It captures the original issuer
before copying userspace arrays, resolves the copied values once under display
locks, and initializes output destinations after dropping those locks. Each
attempt is built from the retained references and current display state, with
file authority and property attachment checked again. The complete driver check
then determines all affected outputs, including any added by the driver.

If readers still need to finish submitting work, the attempted state is
discarded and display locks are released before waiting. Rebuilding never looks
up a framebuffer identifier, blob identifier or input-fence descriptor again.
The original issuer remains responsible for excluding acceptance after lost
authority. Events and output descriptors are prepared only for a checked attempt
whose preparation is ready; signaling failure unwinds those resources before
discarding the attempt. The ordinary blocking driver commit still waits for
its normal native execution dependencies.

The adapter accepts only the supported properties listed below, plus controller
output-fence destinations. Other properties, including writeback and private
properties, return an unsupported-operation error rather than using an unsafe
fallback. Nonblocking, TEST_ONLY and explicitly ticketed requests keep the
ordinary ioctl path. An explicit null ``PREPARE_FD`` is still an explicit
protocol request and is not replaced with an internally created ticket.
Devices without preparation retain their ordinary atomic behavior.

Rebuilding a blocking kernel request
-----------------------------------

An internal caller may describe its operation to ``drm_atomic_commit_request()``
instead of assembling an atomic state that must survive a wait. The helper
owns its modeset lock context and allocates a fresh atomic state for each
attempt. Its callback applies the requested operation to that state using the
ordinary atomic getters. The callback does not install anything itself.

A callback may instead return ``DRM_ATOMIC_REQUEST_UNCHANGED`` after establishing
under the appropriate locks that no display change is needed. The helper then
releases its attempted state and any retained ticket without checking or
installing an update. That result is successful completion of the operation,
not an error and not permission to skip checking a requested display change.

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

A request made on behalf of revocable authority uses
``drm_atomic_commit_request_owned()`` with a retained issuer identity. Every
rebuilt ticket belongs to that same identity. Revoking the issuer wakes a pending
preparation wait and excludes installation through a reservation that was ready
before revocation. Retaining the issuer does not prolong its authority, and
reacquiring display ownership must not replace the issuer of an outstanding
request. The entry requires preparation support rather than silently accepting
an ordinary commit without that protection.

The builder still validates the selected objects and any permission not covered
by issuer revocation. Reporting that no change is needed installs nothing and
does not certify current authority. Rebuilding briefly retains two tickets to
keep admission held across attempts, so the issuer's ticket budget needs room
for both. Allocation failure rejects the request and releases its holds.

The entry is independent of DRM files and userspace descriptors. A
userspace adapter must obtain the issuer through the file's authority policy,
retain resolved input objects and revalidate the selected display objects. The
request helper alone does not make legacy ioctls preparation-aware.

Preparing completion metadata
----------------------------

``drm_atomic_commit_request_with_callbacks()`` also accepts a pair of callbacks
for completion metadata, such as events and output fences. Its build callback
has the same rules as the other request entries. An optional issuer selects the
same revocable authority contract as the owned entry.

The first signaling callback runs only after the attempted state has passed
checking and preparation is ready. It may attach completion metadata, but must
not change the checked display configuration or wait for readers. The driver's
blocking commit runs next if setup succeeds. The second callback then receives
whether that commit succeeded, allowing the caller to publish its prepared
resources or release them on failure. Publishing an output fence does not mean
that the work represented by the fence has finished.

``drm_atomic_submit_request_with_callbacks()`` uses the same preparation and
signaling rules, but asks the driver to finish the accepted commit
asynchronously. It may still wait for preparation before submission, outside
modeset locks. Success means that the driver accepted the update, not that it
has been presented. A pending earlier commit may return ``EBUSY``; the runner
releases unused signaling resources and leaves retry policy to its caller.
That entry supports legacy flip adapters whose completion is reported by an
event. It does not change the ordinary nonblocking atomic ioctl into a waiting
operation, and the existing blocking command entries still wait for native
commit completion.

Every signaling setup call has exactly one matching completion call, including
setup that fails after allocating only some resources. Both callbacks run before
the attempted state and its modeset locks are released. A preparation wait does
not invoke either callback; rebuilding does not keep unused events or output
descriptors alive across that wait. An unchanged request or an earlier error
also invokes neither callback. The runner requires the pair together so callers
cannot provide resource setup without the corresponding completion path.

The ordinary ioctl's private signaling helper owns each event it creates,
including an event whose reservation or output fence setup fails. Rejection
returns reserved event space and unused descriptors, while acceptance leaves
events with the display commit and installs the reserved output descriptors.
An output that was off and remains off cannot request an event or output fence;
that condition is checked when preparing the metadata as well as by the normal
atomic checks. The helper does not alter the checked display configuration.
The blocking retained-request adapter arranges the saved userspace destinations
and calls signaling setup only after preparation is ready.

Retaining inputs from userspace
------------------------------

The ordinary atomic ioctl copies its object identifiers, per-object property
counts, property identifiers and values before applying any assignments.
Lock retries use those copies rather than reading the arrays again. Counts must
fit without wrapping, empty arrays are not read, and repeated objects keep their
separate ordered groups of assignments. Userspace must keep its inputs stable
while they are being copied; copying separate arrays is not a simultaneous
snapshot of memory that another thread is modifying.

Those copies still contain identifiers and descriptor numbers, not owned
resource references. Nonblocking, test-only and explicitly ticketed ioctls
resolve resources while building each attempt. Blocking requests that prepare
internally instead retain the actual objects and fences through the private
adapter described below, and revalidate authority when rebuilding. Copying the
arrays alone does not establish that lifetime. Completion metadata remains
separate from both.

The private ioctl value adapter resolves one supported assignment into the
same typed entries used by kernel requests. It takes a reference to the actual
framebuffer, blob, selected display object or input fence, rather than retaining
its numeric handle. Its temporary value reference and a request's reference
are independent. A failed resolution leaves its destination untouched.

The adapter accepts only properties understood by retained-request replay.
It does not guess whether a private numeric property hides a resource handle.
It also does not resolve preparation descriptors or output pointers.

The private request adapter assembles those values under the display locks.
It retains every target group, including targets supplied with no properties,
as well as an ordered collection of resolved assignments. Repeated targets
remain separate groups and repeated assignments remain ordered. Once assembly
succeeds, changing or freeing the copied arrays does not change the request.
Failure releases all references acquired for the incomplete request.

Controller output-fence destinations are saved separately from that collection
of display values. Each saved destination keeps its controller, attached
property and address; the request's target references keep the controller
available. Copying addresses does not pin userspace memory. A separate helper
initializes every nonnull address to minus one before building attempts and
without holding modeset locks. An inaccessible address rejects the request.

When rebuilding, another private helper checks attachment and includes each
destination's controller in the attempted state. It assigns the last nonnull
address for that controller, preserving the ordinary ioctl's treatment of a
null address as no change to an earlier destination. No userspace memory is
accessed while applying those saved pointers, and no event or descriptor is
allocated. Signaling setup later writes the actual descriptor and still has
to handle a destination that has become inaccessible. Writeback destinations
and preparation descriptors are not accepted by the retained request adapter.

Retaining targets with no assignments matters for permission checks: an empty
group must not become a way to skip checking the caller's lease after waiting.
The private file-authority validator checks that the file is still a current
master, that every retained controller, plane and connector is covered by its
lease, and that retained connectors have not been unregistered. Referenced
controllers are checked separately from assignment targets. It uses retained
objects, not another lookup of the supplied identifiers.

Those checks do not freeze authority. The caller must keep its original issuer
through final acceptance so that master loss or lease revocation after a check
still prevents installation. Applying the request checks property attachment;
the full driver check remains necessary before committing. The blocking ioctl
uses the original issuer throughout those checks and final acceptance.
Preparation descriptors and completion metadata remain separate from the
retained values.

SETCRTC resolves its requested framebuffer, mode and connectors under the initial
display locks. On a preparation-enabled device, it retains those inputs after
dropping the locks and calls the provider's ``set_config_request`` operation.
No userspace connector array or framebuffer identifier is reread after a
preparation wait. The special request to use the current framebuffer resolves
to one retained framebuffer reference, not whichever buffer is current after
waiting. Object references keep storage alive; they do not make pixels immutable.

The standard ``drm_atomic_helper_set_config_request()`` implementation rebuilds
the legacy modeset under display locks, preserving conflicting-encoder handling
and checking the viewport against current plane rotation. Each attempt asks DRM
core to revalidate the selected controller, primary plane when enabling, and
connectors against the file's lease. A connector that has been unregistered is
rejected. The retained issuer prevents installation after master loss or lease
revocation even when those events follow validation.

The operations-table boundary lets DRM core retain inputs and apply file policy
without depending on the separately built atomic-helper module. VKMS and the
Rust CRTC wrapper register the standard helper. A preparation-enabled provider
without that callback rejects SETCRTC with an unsupported-operation error.
Devices without preparation continue using their existing ``set_config`` path.
The new callback is also callable with resolved kernel inputs and a kernel
authorization callback; it neither requires nor manufactures a userspace request.

Legacy SETPLANE uses the corresponding ``update_plane_request`` operation on
preparation-enabled devices. DRM core resolves the plane, controller and
framebuffer once, retains the framebuffer, and passes a ``drm_plane_update``
description without holding modeset locks. The standard
``drm_atomic_helper_update_plane_request()`` helper rebuilds only the selected
controller, framebuffer and geometry, preserving unrelated current plane state.
A null framebuffer disables the plane and clears its rectangles. Every attempt
checks current master and selected-object leases; the original issuer protects
installation from later revocation. Full atomic validation checks the requested
format and rectangles before preparation or acceptance.

VKMS and the Rust plane wrapper register that helper. Providers without the
operation reject SETPLANE while preparation is enabled; devices without
preparation keep their existing plane callbacks. Kernel callers may use the same
resolved helper without a DRM file.

``drm_atomic_set_legacy_flip()`` records a primary framebuffer replacement
without changing the accepted image. It requires an active controller and a
current primary image, preserves geometry and other plane properties, and
rejects pixel-format changes or a replacement too small for the source
rectangle. Full driver checks still validate modifiers and the rest of the
configuration. The attempt forbids modesetting. The caller retains the image
and establishes authorization, preparation and completion signaling separately;
the setter alone does not adapt the legacy page-flip ioctl.

The remembered position used by legacy cursor commands is separate from the
visible cursor plane's rectangle: moving an invisible cursor still affects
where a later image appears. Kernel builders may record that position with
``drm_atomic_set_legacy_cursor_position()``. The atomic helper publishes it
only when installing accepted state under the controller's lock. Discarding or
clearing an attempt leaves the accepted position unchanged. Recording a position
does not update the plane's geometry or by itself adapt a legacy cursor ioctl.

Legacy cursor commands use a separate ``cursor_request`` controller operation.
The core adapter resolves a requested image once, then calls
``drm_atomic_helper_cursor_request()`` with a retained ``drm_cursor_update``.
A move applies to the image current on each attempt; replacing the image without
moving it uses the remembered position current on each attempt. Those implicit
inputs are deliberately read again after a wait, while an explicitly requested
image remains retained. Image hotspots change only in the attempted plane state.
Hiding the image clears its rectangles without forgetting a requested move.

The standard helper runs complete atomic validation and preparation before
accepting the update. Preparation-enabled devices do not select the optional
cursor optimization that changes accepted state in place: normal installation
is needed to account for readers and publish the remembered position together.
VKMS and the Rust controller wrapper register the helper. Devices without the
operation, or without a universal cursor plane, reject legacy cursor commands
while preparation is enabled. Devices without preparation retain their existing
cursor path. Kernel callers use the resolved helper without a file or a buffer
handle; the file adapter rechecks master and controller/plane leases on every
attempt and binds acceptance to the original issuer.

The ``legacy-cursor`` selftest in ``tools/testing/selftests/drm_preparation``
exercises both cursor ioctl versions with real VKMS buffers. It checks hidden
and visible moves, rejection of an invalid image handle, image lifetime after
closing the original buffer handle, and hiding the image. Run it only on an
isolated, single-output VKMS device with preparation enabled. It changes the
display configuration and leaves the output disabled. Reader waits and authority
cancellation are covered separately by the kernel cursor tests.

Rebuilding an ordinary atomic ioctl needs additional input retention. In
particular, an input-fence descriptor number is not a stable identity: another
thread may close it and reuse the number while preparation waits. A request
must retain the resolved fence itself. ``drm_atomic_set_fence_for_plane()``
takes a separate reference for each incoming plane state, leaving the request's
reference alive when an attempt is discarded. It rejects replacing an existing
fence, including replacing it with no dependency. Passing no fence to an empty
state leaves it available for a subsequent assignment.

The ordinary input-fence property adapter uses that setter after descriptor
lookup. The blocking adapter retains the resolved fence separately and uses
the same setter on each attempt. Framebuffer and blob references, selected
objects and output destinations have their own retention and validation rules;
the fence setter alone does not establish them.

On preparation-enabled devices, the older single-property ioctl uses the same
copied-assignment and blocking-commit machinery for supported properties. It
does not require the caller to negotiate the atomic ioctl capability. That
capability is still checked at the atomic ioctl entry point. The adapter retains
the original issuer, resolves resource values once, and rechecks access before
each attempt. Unsupported properties are rejected rather than falling back to
an unprepared commit. The legacy connector power property, ``DPMS``, has separate
semantics and is not handled by that adapter.

Connector power preferences need their own attempted state. Several connectors
may share a controller, and that controller remains active while any attached
connector requests power. ``drm_atomic_set_connector_power()`` records a pending
preference and calculates the controller's activity from pending and current
preferences. It does not publish the preference. Rejection or clearing leaves
accepted preferences unchanged, and checked attempts cannot be edited.

On preparation-enabled devices, normal atomic installation publishes those
preferences under the connection mutex. Explicit preferences remain distinct:
turning one connector on does not change another connector's off preference.
Ordinary atomic power changes, which do not express individual preferences,
still update the legacy power values to reflect controller activity. Later
legacy bookkeeping does not write those values again. The setter is a kernel
building block; it does not itself perform authorization.

Kernel callers use ``drm_atomic_commit_connector_power()`` for a blocking power
command. They retain the connector and issuer, optionally supply a validation
callback, and call without modeset locks. Each attempt checks authority and
recalculates controller activity from the current routing and other connectors'
preferences. The requested preference remains unchanged across preparation
waits. Rejection leaves the accepted preference unchanged.

On preparation-enabled devices, the legacy DPMS property uses that command.
The file adapter retains the original issuer and rechecks ownership of the
display and leases for the connector and its current controller after every
wait. Standby and suspend requests are normalized to off, as with the ordinary
atomic DPMS helper. Turning an unregistered connector on is rejected; turning
it off remains possible while the caller retains authority. Neither the kernel
command nor the file adapter requires a new driver callback.

The ``legacy-power`` selftest exercises off, on and suspend through the legacy
property ioctl on an isolated VKMS output. It inspects controller activity and
connector routing after each command, without requiring atomic negotiation for
the commands themselves. The kernel tests separately exercise pending readers,
authority loss and rejection; the userspace test validates real driver checks
and the normal commit helper rather than simulating those operations.

``drm_atomic_request_create()`` stores an ordered copy of resolved assignments
independently of any attempted display state. Each entry names its target and
attached property. Numeric values are copied; framebuffer, blob, modeset-object
and input-fence values carry their own retained references. Changing the
caller's array or releasing its references after creation does not change the
stored assignments. Duplicate entries remain in order so that request storage
does not silently change property-assignment semantics.

The caller must keep the device's modeset configuration alive. Some display
objects and property definitions have a configuration-wide lifetime rather than
an individually counted reference. Request creation checks device membership,
property attachment and value representation; it neither grants access nor
validates a complete update. Each attempted update must still check authority,
object availability and driver constraints. Driver-private numeric properties
must not conceal resource handles that the request has failed to retain.

Preparation descriptors and output-fence pointers are rejected by this storage
interface. They require separate handling by the transaction or ioctl adapter.
Creating the resolved collection does not acquire admission holds or submit work.

Applying retained assignments
----------------------------

``drm_atomic_request_apply()`` applies the supported retained assignments to a
fresh or cleared atomic update. It first acquires the device's modeset locks
and checks that every property is still attached and has a supported setter.
The caller's required validation callback then checks current authority and
object availability before any assignment is applied. A retained reference
keeps an allocation alive; it does not keep a display lease valid or make a
removed output available again. The callback must check targets as well as
objects named by property values, and the caller must keep that authority
valid through submission.

The setters accept a plane's framebuffer, input fence, controller association,
damage blob, position, size, rotation, color encoding and range, a controller's
mode blob and active state, and the controller selected by a connector.
Controller color tables and matrices are also supported. Resource values use
retained pointers.
Where needed, the attempted state takes its own references. Explicitly assigning
a framebuffer still counts as an update, even when selecting the same
framebuffer. Assignments keep their order: assigning a framebuffer twice selects
the last value, whereas assigning an input fence after a non-null fence remains
an error.

A connector represents an output. Its retained controller value is applied
through the existing kernel setter, which updates both the connector's choice
and the controller's record of connected outputs. A null value disconnects the
output. Clearing a failed attempt releases the references acquired for that
attempt; the request keeps its references for another attempt. The caller must
still validate the connector's availability and permission to use the selected
controller. Keeping a connector alive does not undo its removal or grant access.

Plane position and size use the same setter as ordinary atomic ioctls. Display
positions may be negative; source positions and sizes preserve their fractional
coordinates. Each value must fit the property's range. Checking that the full
rectangle fits the framebuffer and the driver's capabilities remains part of
checking the complete update. Rebuilding applies only the requested fields to
current state, rather than restoring an earlier copy of the plane's geometry.

Plane rotation also uses a shared kernel setter. A request must select exactly
one rotation angle, optionally combined with reflections advertised by that
plane. Unsupported bits and detached properties are rejected without changing
the state. Rotation is reapplied to each fresh attempt; it does not restore
other plane fields from an earlier attempt. The driver still checks whether
the complete combination of geometry, format and transformation is supported.

Plane COLOR_ENCODING and COLOR_RANGE use a shared setter as well. Each value
must be advertised by that plane's attached property; a similarly named private
property does not acquire the standard property's meaning. Rebuilding changes
only the requested fields in current state. This does not select a framebuffer
format or establish that the driver's complete color pipeline supports the
combination; those checks still belong to validation of the atomic update.

The controller's ACTIVE property chooses whether to display its configured
mode. Applying the saved boolean changes only that field; it neither supplies
a missing mode nor checks that the output can run. Repeated assignments take
effect in order, including a final zero that turns the controller off. As with
geometry, the remaining fields come from current state and the complete update
must pass the driver's atomic checks before submission.

Color tables before and after the controller's color matrix, and the matrix
itself, use the same kernel setter as ordinary atomic ioctls. The setter takes
a resolved blob rather than looking up its identifier again. Tables contain
whole color entries up to the advertised size, and a matrix must have the exact
matrix structure size. Selecting a different blob or clearing one records a
color change; selecting the same blob does not erase a change recorded earlier
in the attempt. The driver's checks still decide which color transformations
the complete configuration supports.

Legacy gamma commands also maintain a table returned by the legacy read ioctl.
``drm_atomic_set_legacy_gamma()`` records both the pending color configuration
and an independently retained table for that readback. It selects the standard
gamma property, or the degamma property if gamma is absent, and clears the
other table and color matrix. The input must contain exactly the controller's
legacy table size. Driver-specific gamma callbacks are not supported by the
setter.

The normal atomic helper publishes the cached legacy table during acceptance,
while the controller lock is held. Failed checks and discarded attempts leave
the cache unchanged. Ordinary atomic color updates do not change the legacy
cache, and later color assignments within an attempt do not rewrite the
retained legacy command. The setter does not copy userspace arrays, wait for
preparation or establish authority.

``drm_atomic_commit_legacy_gamma()`` supplies a blocking kernel command over that
setter. Its caller retains the immutable table and issuer, holds no modeset
locks and supplies any additional authorization callback. Each attempt validates
authority and applies the same table to current state before ordinary driver
checking. Waiting discards attempted state, not the requested table.

On preparation-enabled devices, the legacy gamma ioctl captures the original
file issuer before copying any component arrays. All three arrays must copy
successfully before a table is submitted. Attempts use the retained table and
recheck display ownership and the controller lease. A failed copy, check or
preparation leaves accepted readback unchanged. Devices without preparation
retain their ordinary gamma path; custom gamma callbacks are not adapted.

The ``legacy-gamma`` selftest compares requested component arrays with both
legacy readback and the accepted atomic color blob on isolated VKMS. It also
checks that a fault on the third array and a mismatched table size leave the
accepted cache unchanged. It does not enable an output or validate displayed
colors. Pending readers and cancellation are exercised by the kernel tests;
the native test covers actual ioctl copying and ordinary driver acceptance.

Unsupported properties are rejected before any assignments are applied. There
is no fallback that converts references back to numeric identifiers, and no
driver-private property callback is called. Other properties are not supported
yet, nor are the checks required for asynchronous flips. The blocking ioctl
uses the retained collection for supported display values and keeps its output
events and destinations in private completion metadata.

Applying a request does not check or commit the complete display update. If a
setter fails, preceding assignments are not rolled back. The caller
must discard or clear the attempted state on error. Locks stay in the caller's
acquire context, including on failure; a deadlock retry clears state and backs
off those locks before reapplying the request. Every invocation repeats the
authority callback. The immutable request remains available for another attempt.

Shutdown and suspend
--------------------

``drm_atomic_helper_shutdown()`` uses ``drm_atomic_commit_request()`` on
participating devices.
Its operation is to disable every output, which it reconstructs from current
state after a wait. The caller must stop new display producers and retain the
device resources until shutdown finishes. Devices without preparation retain
the ordinary shutdown path.

``drm_atomic_helper_suspend()`` also uses the entry on participating devices.
Each attempt copies current display state for resume before constructing the
disable operation under the same modeset locks. A preparation wait discards the
disable attempt; rebuilding releases its saved copy and takes a fresh one. Only
the copy belonging to the successful disabling attempt is returned. Failure
releases that copy and returns an error rather than a state to resume from.
The saved copy owns its duplicated state and buffer references; it is not a
retained reference to the attempted disable transaction.

The driver must keep display producers stopped throughout suspend and resume.
The ordinary resume helper still resets device state before restoring the saved
copy; it is not an entry for replacing an output that admits new capture reads
while suspended. Provider suspension and restart remain separate integration
obligations before external reader admission is enabled.

Framebuffer removal uses the same entry on participating devices. It keeps a
reference to the framebuffer being removed and finds every plane still using
that object on each attempt. If another update replaces the framebuffer while
preparation waits, removal leaves the replacement alone. If no plane still uses
the old framebuffer, the callback reports that no display change is needed.
The existing fallback to disabling the controller remains available when the
driver rejects disabling its primary plane alone.

Explicit framebuffer removal and file-close framebuffer cleanup both reach that
operation through their shared removal worker. Neither path keeps the file's
framebuffer-list lock across the worker's preparation wait. Closing the file
does not itself relinquish a reader claim; provider teardown still needs its
own policy for stopping readers and accounting for accepted work. A preparation
error prevents the attempted display change. The existing removal wrapper
reports atomic removal failure with a warning; its void interface does not
propagate that error to the removal ioctl. A successful ioctl return therefore
does not establish that a failed display change completed or authorize source
reuse after such a failure.

The in-kernel display client uses the request entry for atomic modesets and
display power changes. Its modeset mutex keeps the requested configuration and
its object references stable throughout rebuilding. The internal master lock
continues to exclude a userspace display owner until the operation returns.
Those two locks remain held during preparation, so a provider must not require
either lock to finish a reader. Modeset locks are released during the wait.
The caller still owns the client's lifetime and device resources.

Checking a client's proposed configuration does not replace display state and
does not wait for readers or hold their admission. Devices without preparation
retain the ordinary client commit path. These client operations do not provide
preparation for legacy userspace ioctls or driver helpers that borrow modeset
locks from their callers.

Kernel tests use an outstanding read claim and a second thread which needs the
modeset lock before releasing it. They check rebuilding, intervening generation
changes, rejected requests and shutdown without depending on a userspace
executor. A second locking test creates a real deadlock between two acquire
contexts and requires a rebuilt request. An interruption test leaves the
original reader unresolved, checks release of the request's admission hold,
and retries after that reader finishes. Suspend tests restore state changed
during the wait and reject failed preparation without returning a saved copy.
Removal tests retain an outstanding reader, exercise controller-disable
fallback and leave a competing replacement installed without an empty commit.
They call the shared framebuffer removal helper, not the file-close or removal
ioctl entry points themselves.
Client tests use a disabled-output configuration to exercise modeset and power
requests, check-only operation and reader failure through the public client
helpers. They test preparation and request ownership, not console rendering or
physical display power transitions.
Issuer-bound request tests revoke authority before ticket creation, during a
pending reader wait and after reservation before state installation. The waiting
test keeps the original reader unresolved and verifies that cancellation releases
only the request's admission holds. Other tests require rebuilding with a live
issuer and reject calls without an issuer or device preparation support.
A contended-lock test presents a pending signal and requires an issuer-bound
request to return without checking or installing display state.
SETCRTC tests call the ioctl handler with a disable request and a pending reader,
drop the file's master authority through the ordinary ioctl dispatch, and reject
providers without the request callback. A separate helper test denies selected
object authorization on the attempt rebuilt after waiting. These tests do not
exercise copying a userspace connector array while a reader is pending.
The test driver installs state through the real swap helper; it does not program
display hardware or exercise native GPU execution.

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
