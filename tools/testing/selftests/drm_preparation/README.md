# A small model of display preparation

CastKMS wants to show a virtual monitor, capture what appears on it, and send
that image somewhere else. The proposed design composes pixels in userspace,
outside the kernel, using a graphics processing unit (GPU). The kernel keeps
track of which output is live, who may look at the final image, and when the
compositor may reuse the storage holding an old picture. Keeping that storage
allocated is not enough: capture must stop reading before its pixels change.

That split only works if permission, preparation and completion stay distinct.

A capture grant is permission to see the finished picture of one output. It is
not permission to change the display mode, and it is not permission to read
the compositor's raw layer buffers.

A preparation ticket is the handshake used when one picture is about to replace
another. It closes the old picture to new readers and waits until work that
was already allowed to start has either finished on the host or been covered
by real GPU completion fences, the signals a GPU driver uses to say "this work
has finished accessing its buffers." Only then may an ordinary display update
be accepted. DRM, the Direct Rendering Manager, is the kernel subsystem that
manages graphics and display devices. Its atomic display updates change
several display objects as one transaction.

A display executor is a trusted local renderer. It is allowed to import source
buffers and submit GPU work. That is a stronger capability than capture. The
proposed kernel interface still validates requests and state transitions. An
ioctl is a request a program sends to a device through an open file. The kernel
does not try to prove that GPU rendering code read only the pixels it was
supposed to read, and it does not revoke ordinary GPU imports at every release.
The executor is trusted the way
a compositor is trusted: if it lies about release, that is a bug in the trusted
display service, not a promised hardware access-control error.

These Python programs are an early design experiment for that handshake. They
are not a kernel driver, not a userspace API, and not a promise that Linux DRM
can implement the same decisions under its real locks. They exist so the
protocol can be argued about with tests before anyone freezes ioctl numbers.

Run them with Python 3, or through the kernel selftest Makefile in this
directory. They need no kernel build, no graphics device, no GPU library, and
no extra Python packages.

```sh
./preparation-model.py
./output-model.py
./pipeline-model.py
```

## What the source model is trying to protect

Imagine GNOME's compositor, Mutter, has put a framebuffer on a virtual
output. A framebuffer describes an image's layout and backing pixel storage;
it can exist without being displayed. Capture wants a copy of the displayed
image. A trusted renderer is allowed to import the source, copy it into private
GPU memory, and later write an independent destination buffer that capture may
export.

The path is therefore source, then a private copy, then a destination. That
extra private copy is intentional. A slow encoder must not keep the buffer
the compositor is currently sending to the display, and a capture recipient
must not keep a live source. Writing the destination in one step is a later
optimization, not a requirement of this model.

When a replacement picture is about to take over, three milestones matter:

1. Preparation closes. The kernel stops handing out new permission to read the
   old source. The trusted renderer has given back every outstanding claim,
   either with "I did not touch it" or with real GPU fences that cover the work
   it already submitted. You can wait on the ticket the way you wait on a
   file. The ticket is not itself a GPU fence.
2. The display update accepts the replacement. Acceptance transfers the seal
   and the fixed list of reader fences to the commit. The GPU readers need not
   have finished yet.
3. The old source is released only after its submitted readers have finished
   and the display no longer needs it. KMS, or Kernel Mode Setting, is the
   part of DRM responsible for display configuration. Its normal completion
   rules still apply alongside the added reader dependencies.

GPU work may finish before acceptance, too. The required ordering is that
preparation finishes before acceptance, and that retirement waits for both
reader completion and the display's release of the old source.

A ready ticket does not mean "the GPU is done." It means no further userspace
action is required to finish retiring the old source. When the ticket becomes
ready, the list of GPU fences that will cover that retirement is frozen; new
fences cannot be added later. Already submitted work may keep running. Using
a released source again without a new authorized claim is an executor bug, not
something this model treats as a kernel-enforced ioctl failure.

While a ticket is live, the kernel holds a seal on the old picture: no new
readers may be admitted. The seal is an owned resource, not a flag that
vanishes when the ticket file is closed. A failed update, or a dry-run update
that only checks whether the configuration would be accepted, must leave that
seal in place. When a real update is accepted, the seal moves to the commit in
the same decision, so there is never a gap where new readers could sneak in.

The model walks through that story one decision at a time. Each method is one
serialized protocol choice: claim a source, submit work, release the claim,
open a ticket, mark the ticket ready, accept it as a commit, or complete the
commit after native fences signal. Python plays the trusted executor here.
When the model says a release covered every submitted fence, that coverage is
asserted, not discovered by inspecting GPU page tables. A stock GPU driver
does not enforce that promise.

## What the tests currently show

Asking to cancel a claim does not finish it. The worker still has to release.
Submission may race that cancellation request, so the ticket stays unready
until the worker reports.

Readiness can be true while the GPU fences are known but not yet signaled.
Accepting the ticket still cannot complete the commit until those fences
signal. Completion also waits for the replacement picture's producer, the
operation that fills its framebuffer. Preparation readiness and that producer
may finish in either order. Neither requires waiting for another userspace
response after acceptance. If GPU work fails, access ends and the error status
is kept; that does not mean the work produced good pixels.

The dry-run check validates the supplied ticket's scope and identity without
waiting for source claims or marking preparation ready. It leaves all model
state unchanged, even when a real commit would still need to wait. A canceled,
consumed, stale or lost ticket is still invalid; a dry run does not revive it.
A rejected real check leaves the ticket's seal in place. Neither kind of
check consumes the ticket or installs a new picture. When
acceptance finally succeeds, the seal moves to the commit in that same
decision. Closing the ticket afterwards must not drop the commit's copy of the
seal.

Two overlapping tickets may cover a shared output. Accepting one makes the
other stale. The winner must not wait for the loser to submit, or two display
updates could deadlock waiting on each other. After every overlapping ticket
is closed, a later claim may reopen the same live picture, but it must still
remember GPU readers that were already submitted.

An earlier accepted update is different from a competing ticket. The model's
test display allows only one accepted, unfinished commit per output. A ticket
for the next picture may become ready while that earlier commit is pending,
but acceptance must wait. Completing the earlier commit does not release any
claims on the next picture: those still belong to the renderer. Closing the
next ticket does not cancel the earlier accepted commit either.

That is a deliberately conservative test-display policy, not a proposed limit
on how many commits real display drivers may queue. The model folds display
completion and retirement into one operation; it does not reproduce DRM's
separate hardware-programming, presentation and cleanup milestones.

If one output in a proposed update is busy, none of that update's outputs
change. Other outputs remain independent. A competing update may invalidate
the waiting ticket in the meantime, so waking after the earlier commit ends
is not permission to submit an obsolete request. The tests exercise those
cases and all 24 orderings of predecessor completion, release of the next
source claim, readiness polling and attempted acceptance. Retrying after both
requirements are satisfied must succeed unless the request became invalid.

Retiring the source does not wait for the destination pool. If the renderer's
private storage is full, new source claims stay unbound: they have demand, but
no permission to read the compositor's buffer yet. The old source can still
retire. A slow encoder is not allowed to pin compositor framebuffers by
sitting on destination buffers.

If the trusted worker dies, that is failure of the display service. New claims
stop. Affected tickets fail instead of becoming ready. Known GPU work is not
invented as complete, and unresolved claims remain until they can be accounted
for. Process exit is not GPU completion.

One declared happy-path schedule gives each admitted job a submit and release
turn before the next replacement. It repeats with room for 1, 2, 4, or 8
source jobs at a time. That number is how many admitted source uses may be in
flight. The iteration count is not a measured frame rate and not a fairness
proof. It only shows that preparation does not have to cancel every admitted
job in order to make progress.

A second test enumerates all 40,320 orderings of a small eight-event set:
submit, release, prepare, ready, accept, close, signal, and complete. After every
decision it checks that the seal is retained until the commit owns it, and
that a source picture is not retired while claims or unsignaled fences remain.
It requires an observed ready ticket as well as accepted and retired commits,
so an unreachable readiness assertion cannot silently pass. That is a bounded
experiment, not every possible kernel race.

The model keeps old objects around so those assertions can be written simply.
Those maps are not a proposal for how production should store in-flight work.

## Remembering a request without keeping an obsolete display state

A display request and the state checked for that request are different things.
The request says what the caller wants to change. Checking it asks whether
those changes are valid against the display configuration now. If preparation
needs to wait, that checked configuration may no longer be current on waking.

The model captures a small request before the wait. It copies the requested
output assignments and an integer position property. It also resolves each
framebuffer identifier, producer-fence file number and preparation file number
to the object they name. The resulting request retains those objects, not the
numbers used to find them. Removing a lookup entry and reusing its number for
another object must not silently change a waiting request. Nor may editing the
caller's input array change the copied values. Retaining a framebuffer object
does not make its pixels immutable.

On each attempt to accept the request, the model starts with the current
display state and applies the retained assignments. An independent output may
have changed during the wait; its new state must be inherited, not replaced by
an old checked copy. A competing update to a requested output instead makes
the old preparation ticket stale. That request is rejected. It needs fresh
preparation, not a retry that ignores the stale ticket.

For the test display, the ticket must name exactly the outputs being changed.
The tests deliberately reject a request that adds an unprepared output. They
also reject requests belonging to another model device or an old worker
epoch. A dry run still changes no preparation state, and rejection before
acceptance leaves the ticket available when it remains valid. Once acceptance
consumes the ticket, trying the request again is rejected even if the caller
never received the first result.

The right to change the display may also move to another authority during the
wait. The model gives each such authority a new generation. A takeover cancels
the old authority's outstanding tickets and removes their seals. Retaining a
request or a ticket reference does not preserve permission to submit it. A new
authority may prepare the same picture afresh. Work accepted before takeover
is different: its retirement guards and GPU dependencies remain until normal
completion. Changing who controls the display cannot make readers finish.

That is a deliberately small authority model. New calls after takeover are
assumed to come from the replacement authority. It does not model DRM files,
leases, simultaneous clients, a period with no master, or the separate capture
grant policies. It tests the boundary between unaccepted and accepted display
work, not a complete implementation of those policies.

Those are tests of retained inputs and fresh reconstruction, not a complete
blocking ioctl. Python object references and the model's retained fence and
ticket tables stand in for owned kernel references. The sample rows are not a
parser for untrusted bytes. The model does not yet exercise detailed permission
checks or topology changes during the wait, blob ownership, interrupted waits,
or output event and file-descriptor publication failures. In particular, merely
holding the inputs must not grant permission that has since been revoked.

## Finishing a copy is not the same as producing a valid picture

The fake GPU accepts dependencies only on work already submitted. A dependent
operation cannot finish before those dependencies finish. It deliberately
allows a producer fence to report failure and the subsequent copy fence to
report success. A successful copy might faithfully copy incomplete or invalid
input pixels; the copy's own status does not answer that question.

Source copies depend on the producers retained by the claimed picture. Later
display updates do not change which producers belong to that old claim. The
ticket may become ready as soon as the renderer releases the claim with its
submitted copy fences, even while producer and copy work remain unfinished.
The old source still waits for native completion before retirement.

The separate staging-status query decides whether the renderer's private copy
is eligible for output. It requires a release report, at least one submitted
copy, successful completion of every copy and successful completion of the
retained producers. Pending work has no final status yet. A no-access release
has no image. Worker loss does not invent a successful report, and a staging
slot already returned for reuse cannot be queried as the old image.

Tests fail the producer before the source claim and while capture is pending.
In both cases a successful copy still gives an invalid staging image, while
the old source retires and the private storage remains reclaimable. Querying
validity is read-only; it does not release sources or complete native fences.

The example retains one explicit producer and any required implicit producers
supplied in a framebuffer snapshot. An implicit producer is a dependency found
through shared-buffer synchronization rather than supplied as the request's
explicit fence. The snapshot is collected once per framebuffer, including
when that framebuffer appears on several outputs. Duplicated references do not
require duplicated status records, but already completed errors remain relevant.

Execution waits and validity evidence serve different purposes. If a later
fence already waits for an earlier one, waiting only for the later fence is
sufficient for ordering. It is not sufficient for deciding whether the earlier
producer succeeded. The model reduces waits through its recorded native
dependency graph while keeping every acquired producer for validity. A test
uses a chain of three operations and an independent producer: the first fails,
the later operations and copy succeed, and staging still reports failure.

The model does not recover errors from reservation histories after their
fences have disappeared or implement real multi-plane layout and attribution.
Nor does it infer ordering from a driver's timeline names. The fake provider
conservatively waits for dependencies even for error completion, rather than
reproducing a particular driver's reset behavior. These are declared ordering
assumptions, not results obtained from a GPU.

## A separate model for exported destinations

The companion program `output-model.py` asks a narrower question: once a
destination buffer has been handed out, who may write it, and does taking the
permission away rewind history?

The buffer's identity survives the end of a stream or grant. The model records
pixel writes when GPU work completes, independently of whether a completion
notification is delivered. Real GPU writes may become visible during execution
as well. Hiding the notification therefore does not undo a write that was
already authorized.

A grant names an authorization scope used to decide which output writes may
be admitted. It is not general permission for a capture recipient to render
into kernel or compositor buffers. An old buffer handle from a previous grant
must not receive pixels from a new, incompatible grant. A write
that was authorized before revocation may still be submitted and completed
afterwards. A new write after revocation must not. The same live grant may
reuse the same allocation for successive frames; that is not a requirement to
allocate a fresh buffer every time.

Each allocation allows only one unresolved output write claim. Claiming it
reserves the storage even before submission; completion makes it available
again. A second claim must wait or choose other storage. Repeating an old
completion cannot free a newer use of the same allocation. That is bookkeeping
for the model's admitted writes, not exclusive access enforced against every
process holding a buffer descriptor.

A destination write may finish with error. Its native access still ends and
the allocation becomes available again, but the model marks its pixels
uncertain and does not deliver it as a valid frame. Failure is not a promise
that an exported allocation was left untouched: a real job may have written
some pixels before failing. The error remains queryable even if notification
was lost. Here the completion method's boolean means valid-frame delivery,
not whether a terminal error record exists.

The joined test retries destination output from the same valid private image
after a failed write. That error does not invalidate the private source copy,
undo scanout retirement, or require a new read of the compositor's buffer.
Both native uses may finish and release private storage before either result
is acknowledged.

The strings that identify those grants are policy labels, not an algorithm
that intersects rights, and not a kernel implementation of access control for
DMA-BUFs, the shared buffers that processes and drivers pass around. The
standalone output model does not
represent consumer read-completion fences, cancellation of an unsubmitted
write, who owns a pool of destination images, whether permission to read a
source also authorizes a destination write, or enforcement of arbitrary buffer
access.

## Following one private image through both authorization decisions

The third program, `pipeline-model.py`, joins the other two models. It imports
their decisions instead of duplicating them. The source and output models
remain independent of that integration, and all three run as selftests.

The joined example starts with a grant for one output and recipient domain.
Its grant factory stands in for an already authorized policy decision, not
for DRM's rules about who may create a grant. Source admission checks that
the grant belongs to the model device, names the requested output and remains
live. Rejecting a claim does not consume queued demand or reserve private
storage. A source claim admitted before revocation may still submit work and
release its native dependencies afterward.

Once the private copy is valid, writing an exported destination is a separate
decision. The grant must still authorize that write, and its scope must match
both the private image and the destination allocation. A source claim that
survived revocation cannot create new output permission. Likewise, a fresh
grant for an incompatible recipient cannot reuse an old private image or
write new pixels into an allocation the old recipient still holds.

An output write claimed before revocation may still finish afterward. Its
private image remains reserved until that write completes, even though the
old scanout source has already retired. Suppressing delivery after revocation
does not undo an authorized write to an already exported buffer.

The destination-stall test keeps a preceding write unfinished. Capture makes
its private copy and retires the source without waiting for that destination.
The private pool eventually fills, leaving further demand without a source
claim, while ordinary display updates keep completing. When the destination
becomes available, the waiting private image is written without rereading a
newer scanout picture. No output operation extends the old reader fence set.

Other tests prevent failed producer content from reaching a destination even
when the copy succeeds, and enumerate all 24 orders of output claim,
revocation, submission and completion. The enumeration requires actual writes
as well as rejected operations, rather than passing by denying everything.

These are serialized admission and lifetime tests. Scope strings stand in for
compatible authorization domains; they do not implement layer attribution,
cropping, DRM leases or permission intersections. The destination model still
uses an explicitly controlled completion operation, not native GPU submission
or consumer read fences. Those distinctions remain part of later qualification.

## Keeping unread results bounded without delaying cleanup

A completion notification may be lost, or its recipient may stop reading. The
output model therefore reserves a result credit before admitting each write.
That credit pays for one record, from pending work through its terminal result.
Finishing the write does not free the credit. Querying its status is
nondestructive, so a caller may retry a query after losing a reply. Only an
acknowledgment of a terminal result frees that credit for another admission.
An early or repeated acknowledgment is rejected, and use identifiers are not
recycled within the result endpoint.

When credits run out, new output claims are rejected before reserving
destination storage. Existing native work still completes and releases its
allocation. A closed result endpoint rejects new writes and discards its
records, but an earlier admitted write may still finish. Dropping delivery
accounting is not evidence that the GPU has stopped accessing the image.

The joined tests deliberately leave every result unacknowledged. They still
retire the old source, recycle the private image and complete more display
updates. A later output attempt fails for lack of result capacity, then
succeeds after one acknowledgment. Another test closes the endpoint while an
output write is unfinished; the private image remains reserved until native
completion, regardless of whether a result will be delivered.

The ledger stores status only, not pixel storage. Its capacity is independent
of private-image depth and is tested at 1, 2, 4 and 8 records. The output
example uses four records by default, not a proposed kernel-wide queue limit.
It does not yet account for capture requests queued before source admission,
per-client quotas, notification file-descriptor installation failures, or
deadlines for requests that never become executable.

The output ledger records native write status. A full capture-request result
will also need to account for revocation and other request-level failures.
For example, native success during revocation is not permission to publish a
new successful capture result; the model suppresses frame delivery but does
not yet implement that complete request-status mapping in the ledger.

## Reusing an image without mistaking identity for content

A valid private image can supply several output writes without another read
of the compositor's storage. The joined model remembers one image per output
and compatible recipient scope. Looking it up requires a live grant and the
same current scene generation. Recycling its private storage removes the
entry. An old image cannot replace a newer cache entry after the display has
advanced, even if the old image remains valid for work already admitted.

Every accepted display replacement starts a new scene generation in the
model. That deliberately includes explicitly selecting the same framebuffer
with unchanged geometry and no damage hint. Tests exercise both an unchanged
producer fence and a replacement producer fence. Neither framebuffer nor
fence identity proves that the pixels stayed unchanged.

Repeated output writes from a cached image leave the source reader set alone.
They remain possible while preparation seals the current scene. Once a new
scene is accepted, the lookup misses; a later capture must supply a fresh
image. Failed, unfinished or incompatibly authorized images cannot enter the
cache. Old entries can keep private storage occupied until recycled, but
cannot acquire new scanout readers.

The example represents content with scene labels, not actual pixels. It
conservatively invalidates on every accepted replacement and does not attempt
damage-based reuse or detect unannounced writes into a current framebuffer.
Nor does it derive ownership adoption from a content update: recipient scopes
remain a separate policy oracle, not an implementation of DRM attribution.

## What passing does not mean

This is an early contract sketch, not the point at which the protocol can be
frozen. The producer example checks explicit retained status, not arbitrary
buffer history or hidden dependencies. Content updates must start a new
picture; unannounced front-buffer writes are not detected. Result credits
cover admitted output writes, not the entire lifetime
of a queued capture request. Request reconstruction covers retained
input identities and current pictures, not the full blocking ioctl lifecycle.
Earlier commits finish through explicit model decisions, not through a kernel
worker or a blocking transaction entry point. They do not explore
every possible interleaving, real DRM locking, hidden GPU dependencies, or
what happens at the exact moment of a crash. They do not allocate production
ioctl numbers or assume a particular GPU driver.

Keep those numbers and driver assumptions out of this directory. Proof on a
real GPU, media encoding, and the Rust virtual display driver are separate
later tracks. A green Python run here is permission to keep talking about the
handshake, not permission to ship it.
