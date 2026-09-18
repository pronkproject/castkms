.. SPDX-License-Identifier: GPL-2.0-only

CastKMS virtual display
=======================

CastKMS is a virtual display that can compose images in the kernel or delegate
complete scenes to an authorized userspace renderer. The Rust driver provides
a display device and a CPU capture path for kernel callers and authorized
userspace clients. Public final-image capture uses generic anonymous capture
files. Renderer source and recipient access use independently fenced jobs;
physical cross-GPU interoperability still requires platform qualification.

Enable ``CONFIG_DRM_CASTKMS`` in a kernel with Rust support to create eight
virtual outputs by default. The ``max_outputs`` parameter accepts one through
eight. Outputs begin disconnected and become visible only after an explicit
monitor attachment. The driver accepts atomic modesetting and linear RGB,
monochrome and YUV framebuffers, including CPU-mappable PRIME imports for HOST
composition. Tiled storage is not supported by the in-kernel compositor.
HOST modes and framebuffer sizes are supported through 8192 by 8192.
Negotiated GPU execution has a static envelope through 16384 by 16384;
the accepted renderer constraints entry determines actual acceptance. Neither
bound is a receiver or transport policy. Cursor planes, eight
shared overlays and per-plane color pipelines are enabled by default. Their
module parameters allow disabling them for focused testing.
The virtual parent has DMA addressing configured before DRM registration so
exporters can map imported attachments. Import retains the exporter's storage
and reservation without requiring a persistent CPU mapping. It does not add
new scanout formats or modifiers, authorize capture, or qualify a real GPU's
allocation and synchronization path.

DRM's software timer supplies a display clock at the programmed mode's refresh
rate. Ordinary flip events are armed after the accepted scene is published and
complete on a subsequent timer tick. Initial activation or an unavailable
clock uses DRM's immediate-event fallback. The timer reads no pixels, and a
flip event does not mean that a receiver has displayed a frame. Receiver and
encoder frame-rate limits do not control the display clock.
The normal device path does not schedule pixel reads on its own. An internal
capture adapter drives composition only when a kernel or userspace caller
requests a frame through an authorized stream. Normal DRM operations on a
caller's own buffers are not a capture capability.

An explicitly authorized service can replace the development monitor and
publish attachment and EDID state through a narrow capability file. There is
no CEC, writeback or CRC collection. Cursor and overlay planes support software
composition. With ALSA support enabled, attached audio sinks also expose
playback and separately authorized audio capture. No default framebuffer
console client is started. Do not use the
development driver for production casting until the required facilities have
been implemented and qualified.

Audio playback and capture
--------------------------

``CONFIG_DRM_CASTKMS_AUDIO`` defaults to enabled when ALSA and its PCM core are
built into the kernel. Each attached monitor with audio capabilities in a CTA
EDID extension or a DisplayID CTA collection gets a playback-only ALSA card.
Applications and sound servers can send stereo, signed 16-bit little-endian
samples at 48 kHz to that card.
The driver exposes the sink's ELD audio description, a stereo channel map and
a jack control. It does not create an ALSA microphone or perform resampling.

Audio capture requires its own explicit capability. The current top-level DRM
master calls ``DRM_IOCTL_CASTKMS_CREATE_AUDIO_CAPTURE`` with the exact CRTC and
connector it controls. The request points to an output structure containing
two close-on-exec descriptors: a read-only audio stream and a revocation file.
Neither image capture nor monitor control or a renderer endpoint implicitly authorizes
audio. The service can transfer the audio file while retaining the revocation
file. Closing the issuing DRM file or the last revocation-file reference ends
the stream, even if another process still holds an audio-file reference.

``DRM_IOCTL_CASTKMS_AUDIO_QUERY`` on the audio file reports the format, queue
capacity and dropped-frame count. Ordinary ``read()`` calls return complete
four-byte stereo frames. The stream has its own ten-millisecond clock and
supplies silence while playback is idle and the CRTC is active. Nonblocking
reads return ``EAGAIN`` when no complete frames are available; ``poll()`` waits
for data or termination.
The queue holds at most 65,536 frames. Overflow discards old queued audio so a
slow reader does not accumulate unbounded delay, and delayed timer callbacks
perform at most forty milliseconds of catch-up work.

Only one active audio stream can capture an attachment. Master loss suspends
the retained capability with ``EAGAIN``, retires its tap and discards queued
samples. This leaves the attachment available to the new master. If the bound
``drm_master`` later becomes current again, the same descriptor opens a fresh
tap; no samples from its old interval survive. Detach, replacement, device
removal and explicit revocation remain terminal, and a retained descriptor
never follows a new attachment. Disabling the CRTC suspends frame delivery and
discards queued samples without revoking the audio capability. Re-enabling
allows delivery again; applications must prepare interrupted ALSA playback
before restarting it. A black image on an active CRTC does not stop audio.
Existing ALSA files are disconnected on detach without waiting for their
owners to close them.
Audio capture does not depend on whether video is
composed in the kernel or by a userspace renderer.

The file operations adapt an independently callable kernel audio provider;
kernel clients do not construct userspace ioctl requests. These experimental
facilities use ``kernel::sound::pcm`` for safe ALSA registration, callbacks,
buffer access, controls and notifications. CastKMS owns the virtual playback
engine, sample clocks, timer limits and CRTC interruption policy.
ELD snapshots come from DRM's native EDID parser. The ``audio`` selftest
exercises real ALSA sample delivery, idle silence, descriptor revocation and
attachment replacement on a disposable device::

    make -C tools/testing/selftests TARGETS=drm_castkms
    tools/testing/selftests/drm_castkms/audio /dev/dri/cardN

Two additional tests exercise sound outside the kernel test framework. Run
them only on disposable devices; they attach monitors and change display modes.
``audio-multi`` requires eight CastKMS outputs and no sound server. It plays a
different signal on each output, checks that the signals stay separate, and
exercises repeated modesets, monitor replacement and DRM master handoff::

    tools/testing/selftests/drm_castkms/audio-multi /dev/dri/cardN

``audio-pipewire`` requires one output, a running PipeWire server with
WirePlumber, and the ``pw-play``, ``pw-dump``, ``wpctl`` and ``jq`` commands. It
waits for the new sound device to appear, directs a known signal to that device,
and verifies the samples through the CastKMS audio file. The test sets that
sink's volume to full and unmutes it. It checks recovery after disabling and
re-enabling the CRTC, then repeats playback after monitor replacement::

    tools/testing/selftests/drm_castkms/audio-pipewire /dev/dri/cardN

Virtual monitor control
-----------------------

One monitor-control capability per virtual connector can be issued with
``DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL``. A normal request requires the
exact current DRM master to hold the connector. An explicit administrative
request instead requires ``CAP_SYS_ADMIN`` in the initial user namespace and
works without acquiring, displacing, or depending on DRM master. This lets a
privileged device-discovery service publish monitors while a compositor keeps
modesetting authority, or before a compositor starts. Connectors begin
disconnected and remain disconnected until explicitly attached. A second
capability is rejected while the first remains open for that connector.

Monitor protocol version 1 uses an input-only creation request with a ``files``
pointer to ``drm_castkms_monitor_files``. Both descriptor numbers are copied
before the managed monitor is published and the descriptors are installed.
Failure installs neither file and leaves the connector disconnected;
callers must discard all output bytes on failure, including partial copyout.

The anonymous close-on-exec control file supports only query, attach and detach
operations. Attach accepts either a complete validated EDID or no EDID, in
which case the driver publishes fallback modes with 1920 by 1080 preferred.
Each successful change emits a normal DRM hotplug event. The capability does
not expose DRM objects, framebuffers, capture images, modesetting, or renderer
authority, including when administratively issued. A second
close-on-exec file lets the issuer revoke the capability without retaining its
control endpoint.

The control file itself carries authority after issuance. It can be passed to
the display service and remains usable across later DRM master changes. Final
control-file close or revocation-file close disconnects the monitor and emits
another hotplug event. Device removal instead makes the monitor terminally
disconnected; a retained capability cannot recreate it.

Accepted constraints
--------------------

Each output exposes persistent atomic ``CONSTRAINTS_ID`` state. Generic KMS
listing describes its fixed default and any published renderer backends.
Discovery requires current modesetting authority, but grants no pixels or
renderer authority. A changed selection requires an ordinary compatible atomic
update with ALLOW_MODESET; omission retains accepted state.

The fixed HOST contract accepts CPU-readable linear formats through 8192 by
8192 with checked allocation/layout bounds. Its per-plane records permit
cropping, fractional source coordinates, positioning, and scale ratios from
1:16 through 16:1. Renderer configurations describe independent whole-scene
restrictions, including exact private-pool dimensions and supported
format/modifier/storage tuples, per-plane geometry and YUV restrictions, and
overlapping total-layer and per-role plane limits. The vendor-neutral static
plane envelope also includes packed RGB layouts which only a selected renderer
may accept; its modifier list remains linear. Static plane properties are not
the complete admission contract.
See :doc:`castkms-renderer` for preparation, publication, ordinary KMS selection
and outstanding read lifetimes.

Renderer issuance has distinct desktop and administrative origins. The normal
request comes from the exact current master. An explicit administrative request
requires host ``CAP_SYS_ADMIN`` and binds a non-master helper's endpoint to the
current top-level owner interval without granting the helper modesetting access.
Issuance returns ``EBUSY`` unless a distinct current owner exclusively controls
the selected output.
Owner loss permanently stales that endpoint; creator close and the separate
revocation descriptor remain independent terminal controls.

Final-image capture uses the same two issuance origins through the generic DRM
capture interface. CastKMS advertises administrative capture support separately
from ordinary capture. An administrative helper must be distinct from the
current master, and its grant is bound to the exact CRTC, connector and current
top-level owner interval without receiving modesetting authority. Leased
targets and unavailable or non-distinct current owners return ``EBUSY``.
Owner-interval loss permanently stales the retained grant, while creator close
and the revocation descriptor retain their existing terminal behavior.

Code boundaries
---------------

``display_control.rs`` retains the exact display objects selected under the
current DRM master's control. Its callback checks that control again and
observes the accepted output configuration under the publication lock. It
does not expose framebuffer storage or confer capture or renderer authority.
Checking the current scene's owner is a separate operation: controlling an
output does not establish that its previously displayed pixels belong to the
same master. Capture and renderer policy build on those observations without
making a retained target a continuing permission.

``image_access.rs`` checks who may use an image that has already been composed.
It requires current ownership of the displayed scene, then compares the image's
original output, configuration, dimensions and owner with that current state.
An earlier frame within the same authorized display interval keeps its original
content serial; checking it does not make it a new frame. These checks neither
issue a capture grant nor grant control to a renderer. Keeping them separate
lets callers apply the same pixel-ownership rules without sharing those powers.

``authority.rs`` records native master transitions and distinguishes uninterrupted
intervals of control. Losing and regaining the same master identity produces a
different interval. Those observations require native control to be stabilized
separately; they neither grant access nor replace scene provenance. Exhausting
the interval counter closes tracking rather than reviving an old observation.

``renderer/permission.rs`` binds exact display control to one native master
identity and a separate revocation owner. Renderer workers and jobs retain the
interval in which they were created, so reacquisition can reactivate the
capability without reviving old work. Retained renderer handles do not retain
the issuer's authority after revocation. Their callbacks permit operations without
requiring a capturable image, but expose no source storage, image exports or
execution activation. Revocation waits for authorization callbacks to leave;
owners of individual operations remain responsible for their resource cleanup.
There is no conversion from a final-image capture grant to renderer permission.

``renderer/endpoint.rs`` serializes one immutable configuration and published
native constraints entry per master interval, together with its private image
namespace. Publication checks the optional preparation fence before listing
the backend; ordinary KMS ``CONSTRAINTS_ID`` state selects it. The
accepted native entry, rather than a mutable driver-side route, identifies the
worker for source and capture admission. Endpoint withdrawal prevents new
selection without inventing completion for accepted work.

The selected endpoint claims changed scenes and may retry a scene after
releasing it without access, while source admission remains open.
Each kernel job owns both retained scene metadata and its preparation read
claim; the framebuffer reference alone does not delay source reuse.
The renderer registers its independent private-image backing before job acquisition.
The selected image is reserved before the source read is admitted, and the
release report covers both source reads and private-image writes. Released
content remains in the registered pool without retaining a source read.

The renderer job-acquisition ioctl prepares ordinary source DMA-BUFs and reserves every
descriptor before copying bounded scene metadata. Only the final, infallible
publication step installs close-on-exec descriptors and makes the job require a
userspace release. Failure before publication reports that no access occurred
and returns the queue slot. At most one source job is outstanding. Once a
CPU or submitted report has retained a private-image result, the unchanged
content serial is not claimed again. A no-access report produces no private
image and leaves that scene retryable under a new job ID; preparation holds
and seals still reject new reads.

Renderer job descriptions
~~~~~~~~~~~~~~~~~~~~~~~~~

``RENDERER_ACQUIRE_JOB`` is the only source-job acquisition operation and uses
``RENDERER_RELEASE_JOB`` to resolve the source-to-private job. Acquisition names
an image retained by ``RENDERER_REGISTER_IMAGE``. A renderer should allocate
the advertised maximum of 64 KiB for the result. Insufficient capacity returns
``ENOSPC``; failure does not consume the job or install any descriptors, even
if userspace memory was partially written. Blank states and content already
reported as composed or submitted return ``ENODATA``.

The version-one result contains a job header followed by back-to-front plane
records and output color-op records. Each plane record includes its role,
stacking position, format/modifier, memory planes, fractional ``src_*``
rectangle and signed ``crtc_*`` geometry. Its color-op records describe the
selected sRGB curves and sign-magnitude matrices. Output records describe
degamma and gamma tables and the output matrix. Equal stacking positions
retain KMS plane creation order. Pixel alpha is premultiplied, sampling is
nearest neighbor, and output color operations follow plane composition.

Metadata is bounded to 24 KMS planes, four memory planes per image, sixteen plane
color operations and 256 entries per output lookup table. These transport
bounds do not advertise additional KMS planes or enable new renderer backends.
All buffer descriptors and the combined acquire fence remain tied to one
source-read claim. The renderer must check acquire-fence success before
reading, then release with no access, completed CPU access or a submitted
``release_fence_fd`` covering source reads and private-image writes. Renderer protocol version 1
publishes version 1 constraints entries for KMS selection. See
:doc:`castkms-renderer` for private-image registration and the independent
private-image-to-recipient output jobs.

The job's ``acquire_fence_fd`` is a sync-file for the exact producer dependencies
captured when KMS accepted the state. An already failed producer rejects
acquisition with its completion error; pending producer work remains represented by
the retained native fence and does not make descriptor preparation wait.

Release distinguishes no access, completed synchronous CPU access and submitted
native work. The submitted form transfers a concrete sync-file fence and a
promise that no later access will be submitted under the job. Dropping a
published job without a release instead records terminal service failure.
Source descriptors are non-revocable storage references; retaining one after
release grants no access to a later content generation.

``castkms.rs`` owns the virtual parent device and DRM registration. Destruction
unplugs DRM and shuts down atomic state before releasing the parent. Display
objects may remain allocated while existing DRM references are being released;
their data does not borrow the module's registration storage.

``monitor.rs`` owns the kernel-facing monitor state machine. Outputs begin
disconnected; its exclusive control object publishes and removes an attached
monitor independently of how that object is transported. ``monitor_file.rs``
is only the UAPI adapter:
it checks master authority when issuing an anonymous capability and translates
validated requests into control-object operations. Neither layer owns a
capture stream or renderer permission.

``display.rs`` defines the output and its atomic checks. ``gem.rs`` defines the
private buffer payload and opts into local allocation. Its optional storage
budget is charged in that payload until the final native allocation reference
is released, including references held by mappings or DMA-BUF exports. A
caller owns the budget independently of any particular buffer; ordinary dumb
allocations and imports do not consume another caller's credit. Neither layer
calls a capture file adapter. Shared DRM helpers remain responsible for object
allocation, framebuffer validation, atomic transaction locking and ordinary
ioctl handling. Future capture policy must not turn buffer allocation or the
ordinary commit path into an implicit grant of access to another client's
pixels.

``scene.rs`` retains the primary plane's framebuffer allocation and copied
source, destination and output geometry independently of the atomic callback. The
source coordinates keep their original fixed-point representation. Atomic
validation prepares geometry in the candidate plane state; the CRTC flush
callback publishes the description together with the accepted state's source
accounting before flip completion. Test-only
and rejected transactions do not publish a replacement. An inactive
controller clears the description. An active controller without a visible
plane publishes a blank scene with no framebuffer or producer dependencies.
Recommitting the same framebuffer
still replaces the description; framebuffer identity is not a content cache.

Each checked plane update that publishes a visible image derives a content
serial from that plane's last accepted atomic state. The serial is installed
only if the update is accepted; test-only submissions and failed candidates do
not consume numbers. Rechecking one candidate derives the same successor
rather than incrementing it again.
Blank updates preserve the counter, so reactivation continues the sequence.
Exhaustion rejects candidates that publish a visible image with ``EOVERFLOW``
rather than reusing a serial; blanking remains possible. The serial is
internal and meaningful only within one plane lifetime.

Active blank scenes have no framebuffer content serial. Their historical
owner is established when visible content is removed, the controller is
activated, or its mode changes. An unchanged blank preserves its accepted
owner, including an unknown owner, across master changes. Current capture
permission is checked separately; a new master does not gain access merely
by submitting an unchanged blank configuration.

The serial conservatively advances for every accepted visible-plane update,
even when the framebuffer and geometry are unchanged. It marks a possible
content change, not proof that pixels differ or that rendering succeeded.
It does not identify the framebuffer's creator, adopt a new capture owner,
or replace authorization.

The output holds at most one description and its source accounting generation.
An accepted update that leaves a visible primary plane unchanged retains its
image description but gets a fresh generation. Active blank updates publish
their separately attributed scene with fresh accounting as well.
Replacement permanently closes the old generation's read admission and releases
its references outside the output lock. Module teardown permanently closes the
output before releasing DRM registration, so an outstanding commit cannot
restore its framebuffer reference after shutdown begins. That ordering breaks
the reference cycle between the output, framebuffer and DRM device.

An owned allocation does not preserve its pixels against later writes. The
description retains historical ownership resolved during atomic validation,
but that attribution is not current permission to capture. Framebuffer
preparation retains the explicit producer fence and the selected reservation
dependencies before DRM's commit helper waits and drops its fence. Explicit
sync selects mandatory kernel dependencies; implicit sync also selects writers.
Every framebuffer memory plane participates, without requiring a CPU mapping.
The native wait fence is merged from those same records, not from a second
snapshot. Merging may omit completed errors and redundant timeline records,
so the description independently retains the original acquired records.
A producer error remains observable after an otherwise accepted display
commit; completion alone does not establish valid pixels. A new update does
not inherit the previous update's records, including on a same-framebuffer
recommit. An empty collection is missing synchronization evidence, not a
success result. Test-only validation does not run framebuffer preparation.

The internal CPU read path takes a claim against the published generation and
rechecks that it is still current before reading. That claim establishes a
read lifetime, not permission to capture. The source claim itself exposes no
pixels to userspace; capture delivery establishes authorization independently.
A later reservation scan cannot recover producer error history discarded
before collection.

The shared Rust reservation interface provides read-only, usage-filtered
snapshots through GEM objects without mapping pixels. It retains individual
acquired fence records independently of the reservation owner, but neither
captures already-signaled error history nor closes admission of later work.
CastKMS uses those snapshots during framebuffer preparation. Drivers that do
not supply the optional Rust preparation callback retain the native GEM
helper's ordinary implicit waiting.

The shared Rust fence interface can place an existing completion record in an
owned sync file. Such a file lets userspace wait for submitted work and inspect
its success or failure using the established Linux synchronization interface.
Creation does not wait, install a descriptor or combine away error records.
Closing the file does not signal the underlying work. A pending record stays
pending until its producer completes it, and poll readiness alone does not
establish that the pixels are valid.

Extraction also works from an already-held file: native type validation returns
an independently retained fence or rejects a file of another type. Kernel
callers do not need to install a descriptor and look it up again. The descriptor
lookup interface uses the same operation after acquiring the file. Rust accepts
a thread-local file borrow because extraction does not use file-position state;
it neither waits nor discards a completed producer error.

That transport helper grants no source access. Renderer source handoff adds
authorization, source lifetime management and close-on-exec descriptor
publication after fallible setup. It must not turn preparation readiness or a
userspace promise to submit work into a DMA fence.

Private host composition
------------------------

The kernel has a bounded software compositor for HOST execution. It accepts
the linear source formats, layer geometry and color operations described above,
using native CastKMS shmem or CPU-mappable imports. Foreign imports are not
usable merely because their format says linear. Checked per-format layouts
bound offsets, row pitches and the last accessed byte of every memory plane.
Private results remain packed XRGB8888 regardless of source format.

Each host pool contains two private images, each at most 256 MiB. Their complete
allocations start cleared, and neither a GEM handle nor a DMA-BUF export is
available through the image interface. A worker reserves a free image before
taking any claim on the displayed source. If both images are occupied, it
reports busy without waiting for reuse or retaining source access.

Private images also share a 512 MiB budget for their output. Each image keeps
its page-rounded allocation charged until the storage is released, including
when a caller retains an image after its pool closes. Replacement pools must
use the same budget. If older images leave insufficient room, allocation
reports busy rather than waiting for their readers. An unsuccessful pool
allocation returns any bytes reserved for its partially constructed images.
Closing a pool therefore prevents further reservations without making retained
storage disappear from the accounting.

Source mapping resources are prepared before admission. The read callback then
checks the retained producer results and copies into the private image outside
display, publication and reservation locks. Returning from the callback releases
the claim only after imported CPU-access intervals have been finalized.
Native mapping cleanup follows outside the claim; it can acquire the buffer's
reservation lock.
A completed private image retains its layout, content serial and attribution,
but not the source framebuffer, mapping or claim. Keeping that image therefore
does not prevent the compositor from reusing its source buffer.

Public HOST destinations are single-plane linear XRGB8888. Each registered
allocation and described image span may be at most 512 MiB, with 16 names per
client. That limit covers every packed HOST mode and permits ordinary pitch
padding; a larger bounded allocation may also back a smaller described view.
All clients share the separate 512 MiB recipient-storage ledger, so current
contention can still reject registration with ``EBUSY``.

Delegated capture queues bind directly to the accepted worker. Their
private-image-to-recipient jobs retain both allocations through native
completion without extending a compositor source read.

``host_compositor/worker.rs`` coalesces queued requests onto one work item and
retains the output publication, pool and latest attempt. Its unique shutdown
owner rejects further requests, drains the work and closes the pool. Copying
and buffer destruction run outside the worker's result lock. The lower
composition, pool and image modules do not depend on the worker or device
registration.

Request callers hold a separate handle rather than the shutdown owner. Each
call to ``request_outcome()`` queues work and returns its own observer. An
attempt covers the requests present when it starts, so a request arriving
during a copy needs a later pass. Requests waiting for the same attempt may
all observe its outcome; one caller does not consume another's notification.
Each observation returns the newest completed attempt that covers the request,
not a permanently assigned frame. The observer retains no read claim.

An observer may check without waiting or wait interruptibly. Worker shutdown
wakes every observer with ``ENODEV``, and interruption leaves observation
available for a retry. Waiting is consumer work outside display, reservation
and worker lifecycle locks, with no source claim. Waking on shutdown does not
certify that the owner's separate source-work drain has finished. An
untracked outcome is a single consumable result for internal
inspection; it is not an independent completion for each queued request.

Dropping a request observer or a shared handle does not stop execution.
Dropping the unique owner rejects further requests through every surviving
handle, discards cached results and drains work. An image already obtained by
a caller keeps its private storage independently of that shutdown. These
interfaces remain internal and grant no permission to deliver pixels to a
capture recipient.

The worker separately retains its last complete image. Reading that cache does
not consume the latest attempt, and a failed attempt does not erase the cached
pixels or suppress the failure. A successful image replaces the cache, including
an active blank image. An attempt with no scene and shutdown clear the cache.
Images are immutable and reference-counted, so
the cache and its readers share one existing pool slot rather than allocating
duplicate pixel storage. The slot and its allocation charge survive until the
final reference is released.

The cached image is historical, not a statement that its content or owner is
still current. An accepted framebuffer update may have occurred since it was
produced, even when the framebuffer object is unchanged. A consumer must
establish current content and recipient permission separately before using
cached pixels as a capture result.

Device ownership includes a lazy host configuration. Creating the device
allocates no image pool and queues no composition. An internal caller supplies
a validated packed-image layout when it needs a worker. Requesting the same
layout returns another handle to the existing worker and preserves its latest
result. A different layout closes and drains the old worker before allocating
the replacement against the output's shared storage budget. If allocation
fails, the configuration has no current worker and the caller may retry after
retained images are released.

An internal caller may also stop an idle worker without closing the device.
That operation drains queued work and releases unused private images, while
leaving the displayed scene and future configuration available. Old handles
remain closed even after another worker starts. Completed images still held by
callers retain their storage charges, so stopping and restarting cannot bypass
the output budget. Releasing a worker does not activate a userspace executor or
change which framebuffers the display accepts.

Configuration and shutdown share a lifecycle lock that worker callbacks never
take. A separate lock protects access to the current handle; neither image
allocation nor waiting for a worker holds that lock. Device shutdown closes
scene publication and drains host work before releasing DRM registration.
References to configuration or worker handles do not postpone shutdown or
permit restarting work afterward. Already completed private images retain their
own storage without retaining a claim on the displayed source.

The composition helpers do not implement registered capture destinations,
capture authorization, a display clock, or userspace-renderer selection. The
capture and execution layers supply those separate policy checks.
The two-image host pool is a private-storage limit, not a receiver frame-rate
policy or a limit on future GPU queues.

Private CPU capture results
---------------------------

The capture layer has an internal adapter for completing CPU requests from
retained host images. It receives a claimed job from the shared DRM capture
code. The job's result storage cannot be read by a consumer until completion.
Receiving a job does not itself establish permission to deliver a particular
image: the caller must authorize both the image and recipient before using
the adapter. The public capture client uses that same authorization path.

The adapter copies packed pixels and sets the unused fourth byte of every
XRGB pixel to ``0xff`` before completing the job. The visible color components
are unchanged, and no page padding is copied. A size mismatch completes with
the copy error rather than publishing an image. Cancellation or revocation
may still suppress delivery when the copy succeeds. No scanout source is
claimed, and completed consumer results own storage independently of the
private compositor images.

Copying and then defining unused bytes is appropriate only because the CPU
job's storage stays private during both operations. A consumer of an exported
DMA-BUF could observe its contents during the copy. The adapter therefore
accepts private CPU jobs, not arbitrary exported destinations; an eventual
path for shared GPU images needs its own rules for writing and reuse.

Capturing through the host worker
---------------------------------

``capture/host_stream.rs`` combines checked private delivery with the output's
shared worker. A kernel caller first establishes a grant for a particular
master, CRTC and connector. The grantor owns revocation; capture handles do
not. Creating a stream reserves its independent result storage before lazily
configuring a worker on the grant's own device. No caller supplies a separate
device that might accidentally refer to another output.

Calling ``capture()`` on that stream queues one request, waits interruptibly
for an eligible composition attempt, rechecks current access and image
ownership, and completes delivery. The call requires an exclusive stream
borrow, preventing overlapping calls from delivering frames out of order on
one stream. Different streams may share a composition attempt without
consuming each other's notifications. The adapter owns no source read while
waiting, and it copies the completed image outside the display policy locks.

A returned request has a terminal result, which the caller must inspect for
copy failure or revocation during delivery. Failure before delivery abandons
the unreturned request and releases its queue credit. The adapter does not
retry automatically or promise a frame rate. The asynchronous file interface
uses independently queued attempts. An inactive or unpublished output returns ``EAGAIN``.
An active blank scene produces ordinary authorized black pixels. Composition
clears the entire reserved private allocation before reuse, checks the accepted
configuration against the pool dimensions, and releases its synchronous claim
before returning. The removed framebuffer and its producer dependencies are
not retained. Capture delivery normalizes the unused XRGB byte as for visible
frames; no private allocation or padding is exported.

Consumer results retain separate storage rather than the worker's private
images. A stream may therefore retain more completed requests than the
two-image compositor pool holds, without delaying source retirement. Closing
one stream discards its results but does not stop a sibling's worker. Grantor
revocation stops new delivery while preserving completed authorized results
on streams that remain open. Replacing a display configuration requires fresh
streams; replacing the worker permanently closes the old adapter's worker
handle. Reopening explicitly avoids silently moving outstanding work to a
different worker.

These kernel operations remain usable without descriptors. The anonymous capture
client adapts queued host output; selecting a userspace GPU renderer remains an
independent execution feature.

Describing a stream before allocation
-------------------------------------

``Capture::describe_stream()`` reports the authorized output's host-linear
layout before creating a stream. A description supplies the XRGB8888 format,
linear modifier, dimensions, row pitch, visible byte count and maximum
private request count. It reserves no stream or image capacity, starts no
compositor work and holds no source read claim. Reported limits are not a
promise that budget will remain available when the caller opens a stream.
The HOST maximum is reduced for the exact image size so the advertised count
can fit by itself in the 512 MiB private-result budget; for example, packed 8K
reports two requests rather than the protocol ceiling of eight. Concurrent
streams may consume that device-wide budget before creation and produce
``EBUSY``.

The description retains its own capture handle and exact accepted display
configuration. ``Description::create_stream()`` opens only for that pair;
there is no operation that combines a description with another grant.
Permission and configuration are checked before allocation and again when
registering the stream. A modeset, even with identical dimensions, requires
a fresh description. Ordinary source-content updates preserve the layout
without preserving old pixels. Grantor close, creator close and device
shutdown still revoke descriptions retained by a caller.

``host_stream::Stream::from_description()`` connects the checked stream to
the same private compositor used by immediate kernel capture. Descriptions
do not promise that a future source image is valid or host-readable; the
executor still checks that image before composition. No source descriptor or
GPU execution authority is exposed by these operations. The anonymous client
separately assigns offer names for the public description interface.

Grants across device shutdown
-----------------------------

Every grant joins a device-wide collection, even if it never opens a stream.
The collection retains only the shared authority's revocation interface; it
does not know the provider's private policy, capture handles or file
operations. There are at most 256 tracked grants per device, independently
of the limits on streams, image storage and grants created by one DRM file.
Releasing a grantor returns its registration credit even when ordinary
references to the revoked authority remain.

Device shutdown permanently closes registration before revoking the
previously tracked authorities and closing streams and display state.
Revocation and final reference cleanup run outside the collection's lock.
Concurrent shutdown calls wait for the first cleanup pass to finish. An
already-issued grant becomes terminal, including a grant without streams;
new issuance fails instead of creating an authority beyond shutdown.
Completed, authorized results retain their existing lifetime.

The grantor owns the unique token that removes its entry. The collection
retains neither that token nor the grantor, and token destruction revokes
before removing tracking. That separation breaks the retained references
between a device, its tracked authorities and policies referring to the
device when external grant owners close. Provider cleanup must not
recursively close the same collection.

Grants tied to a DRM file
-------------------------

``file.rs`` adapts an open DRM file to the kernel provider. It verifies that
the file itself is the current master, rather than merely sharing a master's
identity, and that the selected CRTC and connector belong to that control.
Grant allocation happens outside the native policy locks. The adapter then
rechecks the file and objects before attaching the grant to the file's close
lifetime. A grant may be issued before an image is displayed; creating a stream
and delivering pixels still require their separate current checks.

Each file owns a bounded collection of at most 64 live grantors. Closing the
file revokes those grants even when callers retain their grantor or capture
handles. Closing a grantor earlier removes its registration and frees that
slot. The collection retains native authority objects, not files or grantors,
so it does not keep its creating file alive. Policy callbacks and final
reference cleanup run outside the collection's lock.

The collection is a provider operation with no dependency on DRM file
operations. Kernel-issued grants need not join it: their explicit grantor
still owns revocation, and losing current display control separately denies
access. The file adapter adds a lifetime restriction, not a new way around
recipient policy. Completed, authorized private results retain the same
revocation behavior described above. The optional shared KMS provider resolves
file-visible IDs and invokes this adapter; the :doc:`drm-capture` transport
publishes the resulting capture and control descriptors. The file adapter
itself does not install descriptors or perform image operations.

Transferring revocation to a control file
-----------------------------------------

The provider's ``control_file.rs`` adapter transfers a complete grantor into
an anonymous file. That file only reports revocation; it has no capture,
mapping or modesetting operations. The conversion returns a kernel file
reference without installing a userspace descriptor. Kernel consumers may
continue using the grantor directly without creating a file.

All references to the same control file share one grantor. Closing a
duplicate does not revoke while another reference remains. Final release
revokes before destroying the grantor and removing its creator registration.
The file therefore keeps the registration's quota occupied until its final
release. Ordinary capture handles retain neither the control file nor its
creating DRM file.

For grants issued by a DRM file, closing that creating file still revokes
even if the control file remains open. Transferring a kernel-issued grant
does not add a creating-file restriction. Previously completed, authorized
results retain their documented lifetime in both cases. Failed conversion
drops the unpublished grantor and revokes it; callers must treat that failure
as terminal and perform conversion outside policy locks.

Poll on the control file reports completion of authority revocation,
including device shutdown. Ordinary display changes or loss of current
display control need not terminate a durable grant; current permission
checks and stream revocation enforce those restrictions separately. The
public grant-creation operation preserves these same ownership rules. The
capture file separately supports image negotiation, registered destinations,
queued output, cancellation and terminal dequeue through :doc:`drm-capture`.

Delivering to registered destinations
-------------------------------------

The initial public path captures through private host storage and then copies
into a caller-owned linear destination. It is not a GPU-to-GPU implementation.
Layout, current permission and known source aliases are checked before output
admission. Request depth and destination registrations have independent bounds;
neither encodes a receiver's frame rate or transport window.

Queue observation and destination access are separate. The bounded native
delivery queue retains private pixels, destination storage and the provider
module while mapping, cache maintenance and copying run outside the client's
queue mutex. Readiness snapshots do not make exporter access nonblocking: a
late implicit dependency may still stall one detached delivery. Other queues
and final file release do not wait for that destination fence.

Cancellation requests are observed again after exporter acquisition and between
rows. Explicit stream destruction returns ``EBUSY`` while detached access is
active; successful destruction acknowledges that stream's writes have ended.
Final file release abandons observation without that acknowledgment. Callers
must not recycle its destination merely because its capture descriptor closed.
Completed results retain their request slots until dequeue successfully copies
the terminal metadata to the caller. No source or private image is exported.

Testing in a disposable virtual machine
---------------------------------------

The userspace smoke tests require the libdrm development headers and library::

    make -C tools/testing/selftests/drm_castkms
    tools/testing/selftests/drm_castkms/execution /dev/dri/cardN
    tools/testing/selftests/drm_castkms/monitor-control /dev/dri/cardN
    tools/testing/selftests/drm_castkms/capture-grant /dev/dri/cardN
    tools/testing/selftests/drm_castkms/capture-output /dev/dri/cardN
    tools/testing/selftests/drm_castkms/modeset /dev/dri/cardN

Choose the Rust CastKMS node explicitly in an otherwise unused test VM. The
modeset test changes display state and requires DRM master access. Do not run
the tests against an active desktop. Without a node argument the modeset test
skips instead of selecting a device automatically. It checks the driver name
and development version before attempting a modeset.

Use ``max_outputs=1`` for these single-output fixtures, or
``castkms.max_outputs=1`` on the kernel command line when the driver is built
in. Cursor, overlay and plane-pipeline support can retain their enabled
defaults. Tests select the primary plane by type and CRTC routing, not by the
total plane count. The separately documented ``audio-multi`` fixture requires
eight outputs.

The execution test discovers the fixed default constraints on a disabled
output through the current master and verifies that a non-master file cannot
list them. Neither file receives pixel access.

The monitor-control test creates the anonymous capability through the current
DRM master and checks that a non-master cannot do so. It verifies descriptor
flags, version discovery, request validation, exclusive issuance, EDID-backed
attachment and explicit disconnection. The capability remains effective after
DRM master handoff. Its final close must leave the connector disconnected, and
a new current master must then be able to issue the next capability.

The capture-grant test exercises public issuance and output descriptions
without reading captured pixels. It checks master-file authority, distinct
close-on-exec endpoints, creator and control close, duplicate control ownership,
read-only request memory, partial output faults and descriptor exhaustion.
Repeated output faults must not leak descriptors or consume the creator's
grant quota.
Its description checks enable an output, change horizontal timing without
changing visible geometry, and disable it again. A distinct accepted mode
interval must produce a new description identity.

The modeset test allocates and maps two local buffers, verifies that a test-only
commit leaves the display inactive, enables the output, and submits 48 flips
including same-framebuffer updates. Each submitted flip must produce exactly
one event. It then selects a mode with the same dimensions and half the pixel
clock and submits two more flips. Counter and timestamp checks compare the
display clock with the selected mode before and after that change and across
disable and re-enable. A test-only scaling request succeeds, while a source
rectangle extending beyond the framebuffer is rejected. Finally the test
disables the output and releases its buffers and mode descriptions.

To exercise imported storage, also enable ``CONFIG_DMABUF_HEAPS`` and
``CONFIG_DMABUF_HEAPS_SYSTEM`` in the guest kernel, then supply the heap::

    tools/testing/selftests/drm_castkms/modeset /dev/dri/cardN /dev/dma_heap/system

That variant also imports a private system-heap allocation and creates an
explicitly linear framebuffer without mapping or reading its pixels. It
closes both the DMA-BUF descriptor and the imported buffer handle before
submitting updates, so the framebuffer must retain the storage through
teardown. The fixed HOST constraints accept the CPU-mappable linear import
through non-atomic modesets, page flips and atomic replacements. A test-only
replacement preserves the active framebuffer; blocking and nonblocking
acceptance select the imported framebuffer.

The test then installs the imported framebuffer on an inactive plane and
activates the output without resubmitting the plane. Test-only, blocking and
nonblocking activation must accept the retained import. Restoring the native
framebuffer allows the ordinary flip tests to proceed. These checks cover import
lifetime and HOST eligibility, not GPU execution, producer dependencies or
capture of imported pixels. A missing or inaccessible explicitly selected
heap fails the test.

The test also submits a test-only framebuffer replacement while the output is
active and verifies that the selected framebuffer remains unchanged. It turns
the output off and on without removing its plane configuration before testing
further flips. Optional ``CONFIG_DRM_CASTKMS_KUNIT_TEST`` tests exercise the
output's resource replacement, blanking and terminal shutdown rules, including
resource destruction that reenters output shutdown. Those tests do not read
framebuffer pixels or establish capture authorization.

Those checks establish ordinary DRM submission behavior and virtual display
clock timing, not receiver presentation, GPU interoperability, capture
authorization, or casting performance.

The separate master-lifetime test exercises multiple real DRM files::

    tools/testing/selftests/drm_castkms/master-lifetime /dev/dri/cardN

It requires permission to transfer DRM master between files, normally supplied
by root in the disposable VM, and a libdrm library providing ``drmModeCloseFB``.
It checks non-master rejection, master handoff in both directions, and
same-framebuffer updates after handoff. Test-only and invalid replacements
must leave the accepted framebuffer selected.

The test distinguishes two ways to release a framebuffer. ``CLOSEFB`` drops a
file's reference without disabling the plane. Closing that file then releases
its buffer handles, while the accepted display state keeps the allocation
alive. Replacement must release that remaining framebuffer. ``RMFB`` instead
removes an active framebuffer from display state. The test also closes the
current master while an image remains displayed, acquires master through a
successor file, and finally disables the output and checks framebuffer release.

These cases exercise the real paths that supply attribution evidence. They
observe ordinary DRM state, not the driver's private scene owner, so passing
them does not establish that historical ownership was resolved correctly.
Those attribution tests do not inspect captured pixels. The separate
``capture-output`` test uses the shared final-image interface to verify pixel
delivery, native reuse-fence lookup, alias rejection, failed-copyout retention,
cancellation and dequeue after revocation. It does not qualify GPU composition
or an installed media pipeline.

With ``CONFIG_DRM_CASTKMS_KUNIT_TEST``, a separate set of kernel tests creates
unregistered CastKMS devices and submits transactions through their real atomic
validation and commit callbacks. These tests inspect the private scene owner.
They check that a same-framebuffer update preserves accepted attribution,
including unknown attribution, after a master change. Test-only and rejected
replacements must leave the owner unchanged; an accepted explicit replacement
may adopt the current master. Master loss preserves historical attribution,
disable clears the scene, and terminal shutdown prevents publication even
when a later atomic tail runs.

The kernel fixture creates real retained master identities but supplies the
creation snapshots and master-change observations itself. It neither opens
userspace files nor establishes native master authority. Its results test
CastKMS policy and publication, complementing rather than replacing the real
file tests above. No pixels are read and no capture permission is granted.

Producer tests also pass fences through the real atomic path. They verify
retained failures both before submission and when native waiting enables
signaling, successful producer retention across a test-only candidate, and
the absence of an inherited error on the next same-framebuffer update. These
tests also cover implicit reservation errors, usage filtering and test-only
exclusion. They do not exercise deferred source reads.

Host tests use private fixture allocations to exercise real pixel copies,
including different source pitches and offsets. They retain completed images
across same-framebuffer updates, check that source preparation can finish while
those images remain in use, and verify producer errors, exhausted pools and
queued-worker shutdown. Those tests do not capture an active desktop or
establish permission for a userspace recipient.

A separate set of host tests checks the complete copy with images from one
pixel to 1920 by 1080 pixels. It includes odd widths, extra bytes between
rows and starting offsets that cross a page boundary. The tests write a
pattern directly into source storage, independently of the framebuffer view
used to read it. They then compose, overwrite the source and close the
device before checking every byte of the retained image. That checks storage
layout and independence from the source; it does not measure sustained frame
rate or qualify physical GPU memory. The comparison includes all four bytes
per pixel in the private image, not a policy for exposing unused color bits
to a capture recipient.

CPU delivery tests check the separately defined unused bytes, copy errors and
revocation. They also retain six completed consumer results, release all
private compositor images and reserve the entire private-image budget again
before reading the results. That checks that a consumer backlog neither owns
the private images nor prevents source preparation. The fixtures own every
source and recipient; they do not establish public capture authorization.

A concurrent progress test replaces two immutable framebuffers while a
separate kernel consumer repeatedly requests images. It checks every pixel
of each delivered image and requires successful delivery during the update
interval, excluding results drained after replacement stops. Transient
admission failures are allowed. The bounded test checks useful scheduling,
not a promised capture frame rate or a userspace media pipeline.

When DRM client support is enabled, the import tests export private dumb
storage through an internal client and import it into a separately registered
CastKMS device. They check allocation identity, framebuffer layout and retained
references through teardown without reading pixels. The shared export fixture
has its own handle-cleanup and invalid-dimension tests. Those VM cases do not
qualify a physical GPU's buffers or a userspace compositor's submission path.

Checking the renderer contract
------------------------------

The native constraints and endpoint KUnit suites exercise immutable backend
publication, ordinary atomic selection, source release, delegated recipient
claims, renderer replacement and HOST selection. These checks use the kernel's
display locks and publication paths; they do not establish physical GPU
interoperability.

The ``renderer-control`` selftest joins the renderer and capture file
interfaces. It CPU-copies a source scene into renderer-private storage, holds
the resulting private-to-recipient claim while completing another source job,
copies the private image into the capture destination and checks the delivered
pixels. It also covers fault-safe descriptor publication, continuous scene
updates, cancellation after a recipient claim, retained source reads and
worker withdrawal with outstanding source and recipient claims, and orderly
return to HOST execution. This is a system-memory renderer fixture, not physical
GPU interoperability evidence.

Building without another display driver
---------------------------------------

A distribution or test kernel may have another display driver built in. Its
dependencies can hide a missing dependency in CastKMS. The build-only
``standalone.config`` fixture keeps other display drivers, KUnit and framebuffer
console support disabled, and builds both CastKMS and the display helpers as
modules. From the kernel source directory, with a working Rust kernel
toolchain, run::

    build_dir=$(mktemp -d /var/tmp/castkms-build.XXXXXXXX)
    make O="$build_dir" ARCH=x86_64 LLVM=1 \
        KCONFIG_ALLCONFIG="$PWD/tools/testing/selftests/drm_castkms/standalone.config" \
        allnoconfig
    make O="$build_dir" ARCH=x86_64 LLVM=1 -j4 bzImage modules

Check that the generated configuration contains ``CONFIG_RUST=y``,
``CONFIG_DRM_CASTKMS=m`` and ``CONFIG_DRM_KMS_HELPER=m``. Kconfig may otherwise
disable Rust when the compiler prerequisites are unavailable, leaving a
successful build that never compiled the driver. The fixture is for build
validation, not a bootable VM test configuration. It deliberately leaves
runtime-test facilities such as an initramfs and serial console disabled.
