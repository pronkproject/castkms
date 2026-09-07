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
signal. If the GPU work fails, access ends and the error status is kept. The
model does not pretend the copy produced good pixels.

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

The strings that identify those grants are policy labels, not an algorithm
that intersects rights, and not a kernel implementation of access control for
DMA-BUFs, the shared buffers that processes and drivers pass around. This
output model does not yet join up with the source model. It also does not
represent two GPU writes overlapping on one buffer, who owns a pool of
destination images, whether permission to read a source also authorizes a
destination write, or enforcement of arbitrary buffer access.

## What passing does not mean

This is an early contract sketch, not the point at which the protocol can be
frozen. The programs do not ask whether a producer's completion fence actually
means the pixels are valid. They do not model an update that reuses the same
framebuffer for new content without starting a new picture. They do not model
credits for completed capture results, rebuilding a request after a wait, or
the kernel resolving an earlier commit in the background. They do not explore
every possible interleaving, real DRM locking, hidden GPU dependencies, or
what happens at the exact moment of a crash. They do not allocate production
ioctl numbers or assume a particular GPU driver.

Keep those numbers and driver assumptions out of this directory. Proof on a
real GPU, media encoding, and the Rust virtual display driver are separate
later tracks. A green Python run here is permission to keep talking about the
handshake, not permission to ship it.
