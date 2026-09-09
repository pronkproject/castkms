=============
DRM Internals
=============

This chapter documents DRM internals relevant to driver authors and
developers working to add support for the latest features to existing
drivers.

First, we go over some typical driver initialization requirements, like
setting up command buffers, creating an initial output configuration,
and initializing core services. Subsequent sections cover core internals
in more detail, providing implementation notes and examples.

The DRM layer provides several services to graphics drivers, many of
them driven by the application interfaces it provides through libdrm,
the library that wraps most of the DRM ioctls. These include vblank
event handling, memory management, output management, framebuffer
management, command submission & fencing, suspend/resume support, and
DMA services.

.. contents::

Kernel-controlled Final-image Capture
====================================

``drm_capture.h`` provides an initial, kernel-only request core for a provider
that already knows which final image its recipient is allowed to receive.
It is not a grant-creation ioctl, a DRM object authorization check, or a way
to export arbitrary scanout planes. The creating caller must establish that
policy before constructing the stream. One stream has one fixed image size
and one unchanged authorization scope for its entire lifetime.

The caller chooses the request capacity and image size. Queue admission
reserves a credit and allocates zeroed private image storage before a provider
claims a request. A full queue returns ``-EAGAIN`` without selecting a source.
Completed results retain their credits until acknowledged, so both queued
requests and unread results remain bounded. Memory use is bounded by the
configured capacity times the image size, plus request metadata; a future
userspace adapter must validate those limits against its own allocation quota.

The synchronous reference provider, ``drm_capture_publish_snapshot()``, serves
the oldest queued request from an already composed kernel image. Its input
must be readable, coherent and authorized, including any padding bytes. The
image size must match exactly. Publication copies the image into independent
storage before returning. Subsequent source changes, display updates or a slow
result reader therefore do not extend access to the original image.

A provider with asynchronous kernel-controlled work instead uses
``drm_capture_claim()``, fills the storage obtained from
``drm_capture_job_data()``, and calls ``drm_capture_complete()`` exactly once
after access ends. Completion consumes the provider's job ownership; the
provider must not use the job or its data pointer afterward. No capture mutex
is held while the provider accesses the image. A claimed job owns an additional
stream reference until completion.

Revocation and claim admission share the stream mutex. If revocation wins,
no new job is claimed. If claim wins, the provider may still finish its earlier
authorized write, but the retained request result becomes ``-EKEYREVOKED``.
Canceling an active request likewise does not make its storage safe to free.
The first recorded cancellation reason wins; an already completed result
never changes. Closing the stream discards queued demand and completed results,
but active jobs keep their storage until the provider acknowledges completion.
Process exit or a timeout is not such an acknowledgment.

``drm_capture_query()`` reports pending or terminal status without consuming
the result. ``drm_capture_copy_result()`` copies only successfully completed
images to a kernel buffer; pending or failed requests leave that buffer
untouched. ``drm_capture_ack()`` releases a terminal result and its credit.
Unlike source storage, the private result image remains allocated until
acknowledgment or close. New streams allocate new storage rather than changing
the authority of old storage in place.

``drm_capture_discard()`` abandons one request when its consumer no longer
wants the result. The identifier immediately stops being usable for query,
copy, cancellation or acknowledgment. Unclaimed and completed requests release
their storage and credit immediately. A claimed request remains allocated,
including its credit, until the provider completes; completion then frees it
instead of retaining a result. Abandoning a consumer is not evidence that its
provider has stopped accessing memory. Other requests and the stream's
permission are unaffected.

The caller's stream reference must remain live for every ordinary API call.
``drm_capture_get()`` takes another reference from one already held;
``drm_capture_put()`` releases only that reference. Dropping an observer does
not close the stream for its other owners. Final release drains queued and
completed requests; a claimed provider job prevents final release until its
completion.

``drm_capture_shutdown()`` is the separate, idempotent decision to end
admission and discard delivery. It may race calls using independently held
references. ``drm_capture_close()`` combines shutdown with release of the
caller's reference. Neither shutdown nor close ends access by an already
claimed provider job. A file adapter must preserve those distinctions when
file references or provider registrations end. None of these operations needs
a userspace ioctl context, so an adapter will call the same core as an
in-kernel consumer.

The KUnit ``drm_capture`` suite exercises the core with real allocations and
kernel-controlled snapshot publication. It does not create a public capture
interface. Capture-holder files, DRM grant policy, destination DMA-BUF registration,
format negotiation, provider notification and safe Rust ownership wrappers
are subsequent integrations. In particular, the CPU snapshot helper does not
claim the delegated GPU composition path or create a future-userspace fence.

VKMS Reference Composition
--------------------------

VKMS supplies a software compositor for testing display behavior without a
physical monitor. Its internal ``vkms_composer_capture()`` entry point exercises
the capture request core with the same plane blending and output gamma
correction used for its display checksums and writeback. The caller supplies
a prepared display state, an authority and a registered stream. The provider
asks the authority to approve a claim before accessing source pixels. An empty
queue or denied claim performs no composition.

Each accepted request receives the completed image directly in its own private
storage. Rows contain VKMS's internal ``pixel_argb_u16`` representation, with
four native-endian, 16-bit components per pixel and no row padding. This is a
kernel test representation, not a public image format or a restriction on
future GPU destinations. Once the synchronous call returns, consuming or
retaining the result needs no further access to the display sources. Capture
does not submit a writeback job or deliver a display checksum.

The caller remains responsible for the relationship between permission and
the selected display state. It must keep source selection stable across the
authority check and composition, and ensure the policy approves that exact
output, content and layout. Matching a byte count alone does not establish
matching dimensions or permission. All source mappings, pixels, plane state,
color operations and the prepared gamma table must remain valid and coherent
until return. Producer work must have finished successfully before entering;
framebuffer references alone neither establish completion nor prevent pixel
reuse. The entry point does not acquire display locks or wait on producers.

Composition success describes the provider's work, not unconditional delivery.
Revoking authority or canceling the request during composition suppresses the
result through the shared request core, without freeing storage that the
compositor is still writing. Consumers inspect the request's terminal status.

The ``vkms-capture`` KUnit suite uses prepared states to exercise retained
pixels, gamma correction, denied admission, size rejection and revocation
during a source read. It does not establish live display scheduling or a DRM
grant policy. The reference entry point has no userspace dispatcher and is not
automatically invoked by VKMS's display worker.

Rust Capture Ownership
----------------------

``kernel::drm::capture`` wraps the private CPU capture core for Rust consumers.
An ``ARef<Stream>`` owns a native stream reference. Dropping one reference does
not stop other owners; ``shutdown()`` is the separate decision to stop admission
and delivery. Creating a stream still requires the provider to establish its
source and recipient policy. The wrappers do not implement a DRM grant policy.

Queueing returns a ``Request`` that retains its originating stream and keeps
its numeric identifier private. Its status and copy operations therefore
cannot accidentally address the same number in a different stream. Dropping
the request discards demand or a retained result. If a provider is active,
storage and credit remain charged until that provider completes. Explicit
cancellation instead preserves terminal status until the request is dropped.

A claimed ``Job`` exposes its initialized image only through an exclusive
borrowed byte slice. Completion consumes the job, so Rust rejects a second
completion or completion followed by access through an outstanding slice.
Dropping an unfinished job reports cancellation after all synchronous borrows
have ended. The wrappers expose no raw job pointer or submission interface:
these guarantees apply to kernel-controlled CPU access, not outstanding GPU
work. A future native-execution wrapper must retain ownership until real
completion rather than using the CPU job's destructor as a GPU fence.

All operations, including destructors, need a context that may sleep. The
``rust_drm_capture`` tests exercise the ownership transitions against the C
core, and the Rust type-check fixtures reject duplicate completion and use of
image storage after completion. Neither test family validates live source
permission or asynchronous native execution.

Waiting for a Capture Result
----------------------------

``drm_capture_wait_result()`` waits interruptibly until the selected request
has completed or its identifier disappears. Waiting does not copy, acknowledge
or discard the image. A zero return supplies a completed result, but that
result's status may describe a failed producer. An interrupted wait or missing
request returns an error without changing the supplied result storage.

An active provider still owns its writable storage after cancellation or
revocation. Waiting for that request therefore continues until the provider
actually completes. Discarding the request instead makes its identifier
unavailable and wakes the waiter with ``-ENOENT``, while the provider retains
storage and queue credit. A wakeup is not a source-release signal.

The stream retains one stable result wait queue. Kernel observers can obtain
it with ``drm_capture_result_waitqueue()``, register before querying and recheck
after each notification. Notifications may concern another request or an
operation that left the observed request unchanged. The caller must retain the
stream for the entire registration and must not hold locks needed by the
provider. The wait helper supplies that register-and-check ordering without
requiring a file, descriptor or userspace dispatcher.

Rust's ``Request::wait()`` retains the originating stream through the request
owner. Its outer ``Result`` describes waiting, and its inner ``Result``
describes the completed image. ``Ok(Err(EIO))`` therefore means capture failed,
while an outer error means the wait was interrupted or the request disappeared.
Successful observation is a snapshot, not a reservation against a later stream
shutdown. Native tests exercise sleeping waiters, cancellation, revocation,
discard and interruption; Rust tests check retained results and the distinction
between wait failure and image failure.

Capture Authority Lifetime
--------------------------

A capture permission can outlive the image size and request queue used for one
mode. For example, changing a virtual monitor's resolution needs a new stream,
but need not require the compositor to grant permission again. Conversely,
revoking permission must prevent new streams even if an old stream has already
been closed.

``drm_capture_authority.h`` provides that independent lifetime. Its provider
context holds the authorized scope, rights and any policy references. Creating
the authority takes ownership of the context only on success. The constructor
does not authorize a display or bypass DRM master and content-ownership checks;
those remain explicit responsibilities of the provider.

Before admitting a resource, the provider calls
``drm_capture_authority_begin()``. Success holds an admission mutex until
``drm_capture_authority_end()``. Policy validation and resource registration
belong inside that interval. Another provider lock may also be needed to keep
the policy being checked stable. A reference to the authority keeps its memory
alive, not its permission valid.

Revocation acquires the same mutex, permanently closes admission, and releases
the mutex before calling the provider's cleanup callback. A concurrent revoker
waits for that callback to finish rather than reporting completed cleanup
early. The callback must not recursively revoke the authority or wait for an
operation that needs revocation to finish. In particular, revocation cannot be
called with the admission guard held.

Cleanup completion means that the provider has stopped admission and initiated
safe resource cleanup. It does not mean that GPU work has completed. Claimed
jobs retain their own references until their actual completion; neither a
revocation notification nor a cleanup callback replaces a native completion
fence. The authority wait queue is awakened after the callback finishes. A
waiter must register before testing the condition to avoid missing that wakeup.

Ordinary references are acquired with ``drm_capture_authority_get()`` and
released with ``drm_capture_authority_put()``. Final release also revokes if
necessary, then releases the provider context. These operations may sleep. A
future file adapter and an in-kernel consumer use the same admission and
revocation operations; closing a mode-specific stream is not an authority
operation. The KUnit ``drm_capture_authority`` suite exercises terminal cleanup,
stream replacement and concurrent revocation without introducing a public ABI.

Registering Streams for Revocation
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

After validating permission under the admission guard, a provider calls
``drm_capture_authority_add_stream_locked()`` to register an already-created
stream. The registry takes its own stream reference on success. Failure leaves
the caller's reference unchanged. Registering the same stream twice in one
authority returns ``-EEXIST``. Providers must enforce their stream limits and
must not share storage across incompatible authorization scopes.

The registry retains the stream, not the other way around. Consequently, the
last authority reference still triggers revocation even when streams remain
registered. At revocation, the core closes admission and detaches the entire
registry under the authority mutex. Outside that mutex it revokes each stream
and releases the registry's references before calling the provider's cleanup
callback. The callback still handles resources other than registered streams;
it must own any stream reference it intends to use itself.

``drm_capture_authority_remove_stream()`` instead shuts down one registered
stream and releases its registration reference. It does not revoke the grant
or affect replacement streams. Call it without the admission guard and with
live references to the authority and stream. A false return means that no
registration was found; a concurrent revoker may already own its cleanup.
Removal is therefore not a substitute for waiting for authority revocation.
Neither removal nor revocation completes an already-claimed provider job.

Registration is cleanup ownership, not pixel permission. Providers must still
check current content authority when admitting source access. File adapters
must use those provider operations rather than treating a registered stream
as a way to bypass policy. The request core remains independent of authorities,
file descriptors and DRM master selection.

Authorizing a Provider Claim
~~~~~~~~~~~~~~~~~~~~~~~~~~~

``drm_capture_authority_claim_stream()`` combines authority lifetime, registered
stream membership and the provider's ``authorize_capture`` callback before
claiming a request. The admission mutex remains held across those steps, so
authority revocation and stream removal cannot intervene between the check and
claim. An unregistered stream is rejected without calling provider policy.
Without a policy callback the operation returns ``-EOPNOTSUPP``; registration
alone never enables capture through this entry point.

The callback returns zero to permit capture or a negative error to deny it.
A positive result is invalid and also denies access. Denial does not consume
a queued request, allowing temporary policy conditions to clear without
recreating the queue. The callback must not submit work or assume a job exists:
an approved claim may still return ``-EAGAIN`` when no request is queued.

The mutex orders authority operations, not every source of display changes.
A provider whose current source or policy is protected by another lock must
hold that lock across the whole claim call and use a consistent lock order
for registration and teardown. The callback must not reenter authority
operations. After success the provider retains the approved source through
its own ownership mechanism; holding a job neither freezes framebuffer pixels
nor retains a modesetting transaction. The provider completes the job exactly
once when its actual access ends, even if permission is revoked meanwhile.

The same claim operation is available to kernel consumers and future capture
adapters. It does not implement DRM master selection, scene attribution or a
GPU source-use protocol. Providers still supply those policies and lifetimes;
the shared operation prevents separating their approval from request admission
with respect to authority revocation.

Rust Authority Ownership
~~~~~~~~~~~~~~~~~~~~~~~~

Rust providers use ``capture::Authority<P>`` for the same native authority,
where ``P`` implements the provider's ``Policy``. Construction transfers an
``Arc`` reference to the native owner. Failure releases that reference without
invoking the authority's revoke callback. Success retains both the provider
and the module that contains its callbacks. The policy trait makes correct
callback-module ownership an explicit safety obligation; the ``vtable`` implementation
attribute selects the implementing module by default.

Dropping the last authority reference revokes and releases its provider.
Explicit ``revoke()`` waits for the cleanup callback but does not release
other owners' references. ``is_revoked()`` observes closed admission, while
``cleanup_done()`` observes the callback's return. Neither observation means
that GPU work has completed. Avoid ownership cycles between a provider and
its authority, and release module-pinning authorities before expecting their
callback module to unload.

``begin()`` returns an admission guard tied to the borrowed authority and the
current task. Rust rejects moving it to another task, releasing its owner
while it remains borrowed, or fabricating a guard without locking. The guard's
``add_stream()`` operation registers cleanup ownership after provider policy
validation. Dropping the guard unlocks admission; it does not revoke anything.
``remove_stream()`` operates outside the guard and stops only the selected
registered stream.

``claim()`` uses native membership and policy checks. A missing
``authorize_capture()`` implementation denies claims. Rust's result type
permits only successful approval or an error, not an accidental positive
integer. The provider still stabilizes its source and live policy across the
call, in the same lock order as the native contract. Registration is not
permission, and an approval callback must not assume that a request exists.

A successful claim returns the existing unique CPU ``Job`` owner. That job
retains its private result storage even if the authority and policy are
subsequently released, while revocation prevents delivering its image. It
does not retain arbitrary provider resources or asynchronous source access.
Those require their own ownership; a CPU job must not stand in for a GPU
release protocol. Runtime tests cover provider lifetime, denied claims,
stream removal and active jobs across revocation. Compiler fixtures check the
admission guard's ownership restrictions without granting real pixel access.

Anonymous Revocation File
------------------------

``drm_capture_control_file_create()`` wraps an existing authority in an
anonymous file for the component responsible for revocation. It does not open
a DRM primary node or grant any additional rights. There is no image, memory
mapping or ioctl dispatcher on that file. A capture-holder file is a separate
interface and is not implemented by this helper.

The file owns an authority reference. Duplicating the file reference does not
create another authority, and releasing one duplicate does not revoke while
another remains. Final file release invokes the same revoke operation used by
kernel consumers, even when other ordinary authority references still exist.
The file therefore represents a decision to revoke, not merely a reference
keeping an allocation alive. Creating two independent control files for one
authority gives either file's final release the power to revoke it.

Poll reports no readiness before the provider's revoke callback finishes and
reports persistent ``POLLHUP`` afterward. Kernel-triggered revocation wakes an
existing poll waiter. The notification says that admission has ended and safe
cleanup has been initiated; pending GPU work may still be draining. An
authority that is already terminal produces the same terminal poll result.

The constructor returns an owned file reference, not an installed descriptor.
Failure leaves the caller's authority unchanged. Once creation succeeds,
discarding even an unpublished file revokes on final release. A future grant
creation operation must reserve descriptors with ``O_CLOEXEC`` and finish
fallible setup before installing files. It must treat rollback of a created
control file as terminal rather than trying to reuse the same grant. There is
no public grant-creation ABI in this helper alone.

Rust providers call ``Authority::create_control_file()`` to obtain the same
owned file without installing a descriptor. The adapter lives separately from
the authority implementation and does not add file dependencies to provider
callbacks. A returned ``ARef<File>`` retains the authority and its policy even
after ordinary kernel authority references are dropped. Cloning that file
shares its lifetime; calling the constructor again creates another independent
revoker. Rust runtime tests exercise both cases, policy retention and file
creation after revocation. Merely creating another file cannot reopen a
terminal authority.

Driver Initialization
=====================

At the core of every DRM driver is a :c:type:`struct drm_driver
<drm_driver>` structure. Drivers typically statically initialize
a drm_driver structure, and then pass it to
drm_dev_alloc() to allocate a device instance. After the
device instance is fully initialized it can be registered (which makes
it accessible from userspace) using drm_dev_register().

The :c:type:`struct drm_driver <drm_driver>` structure
contains static information that describes the driver and features it
supports, and pointers to methods that the DRM core will call to
implement the DRM API. We will first go through the :c:type:`struct
drm_driver <drm_driver>` static information fields, and will
then describe individual operations in details as they get used in later
sections.

Driver Information
------------------

Major, Minor and Patchlevel
~~~~~~~~~~~~~~~~~~~~~~~~~~~

int major; int minor; int patchlevel;
The DRM core identifies driver versions by a major, minor and patch
level triplet. The information is printed to the kernel log at
initialization time and passed to userspace through the
DRM_IOCTL_VERSION ioctl.

The major and minor numbers are also used to verify the requested driver
API version passed to DRM_IOCTL_SET_VERSION. When the driver API
changes between minor versions, applications can call
DRM_IOCTL_SET_VERSION to select a specific version of the API. If the
requested major isn't equal to the driver major, or the requested minor
is larger than the driver minor, the DRM_IOCTL_SET_VERSION call will
return an error. Otherwise the driver's set_version() method will be
called with the requested version.

Name and Description
~~~~~~~~~~~~~~~~~~~~

char \*name; char \*desc; char \*date;
The driver name is printed to the kernel log at initialization time,
used for IRQ registration and passed to userspace through
DRM_IOCTL_VERSION.

The driver description is a purely informative string passed to
userspace through the DRM_IOCTL_VERSION ioctl and otherwise unused by
the kernel.

Module Initialization
---------------------

.. kernel-doc:: include/drm/drm_module.h
   :doc: overview

Device Instance and Driver Handling
-----------------------------------

.. kernel-doc:: drivers/gpu/drm/drm_drv.c
   :doc: driver instance overview

.. kernel-doc:: include/drm/drm_device.h
   :internal:

.. kernel-doc:: include/drm/drm_drv.h
   :internal:

.. kernel-doc:: drivers/gpu/drm/drm_drv.c
   :export:

Driver Load
-----------

Component Helper Usage
~~~~~~~~~~~~~~~~~~~~~~

.. kernel-doc:: drivers/gpu/drm/drm_drv.c
   :doc: component helper usage recommendations

Memory Manager Initialization
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

Every DRM driver requires a memory manager which must be initialized at
load time. DRM currently contains two memory managers, the Translation
Table Manager (TTM) and the Graphics Execution Manager (GEM). This
document describes the use of the GEM memory manager only. See ? for
details.

Miscellaneous Device Configuration
~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~

Another task that may be necessary for PCI devices during configuration
is mapping the video BIOS. On many devices, the VBIOS describes device
configuration, LCD panel timings (if any), and contains flags indicating
device state. Mapping the BIOS can be done using the pci_map_rom()
call, a convenience function that takes care of mapping the actual ROM,
whether it has been shadowed into memory (typically at address 0xc0000)
or exists on the PCI device in the ROM BAR. Note that after the ROM has
been mapped and any necessary information has been extracted, it should
be unmapped; on many devices, the ROM address decoder is shared with
other BARs, so leaving it mapped could cause undesired behaviour like
hangs or memory corruption.

Managed Resources
-----------------

.. kernel-doc:: drivers/gpu/drm/drm_managed.c
   :doc: managed resources

.. kernel-doc:: drivers/gpu/drm/drm_managed.c
   :export:

.. kernel-doc:: include/drm/drm_managed.h
   :internal:

Open/Close, File Operations and IOCTLs
======================================

.. _drm_driver_fops:

File Operations
---------------

.. kernel-doc:: drivers/gpu/drm/drm_file.c
   :doc: file operations

.. kernel-doc:: include/drm/drm_file.h
   :internal:

.. kernel-doc:: drivers/gpu/drm/drm_file.c
   :export:

Misc Utilities
==============

Printer
-------

.. kernel-doc:: include/drm/drm_print.h
   :doc: print

.. kernel-doc:: include/drm/drm_print.h
   :internal:

.. kernel-doc:: drivers/gpu/drm/drm_print.c
   :export:

Utilities
---------

.. kernel-doc:: include/drm/drm_util.h
   :doc: drm utils

.. kernel-doc:: include/drm/drm_util.h
   :internal:


Unit testing
============

KUnit
-----

KUnit (Kernel unit testing framework) provides a common framework for unit tests
within the Linux kernel.

This section covers the specifics for the DRM subsystem. For general information
about KUnit, please refer to Documentation/dev-tools/kunit/start.rst.

How to run the tests?
~~~~~~~~~~~~~~~~~~~~~

In order to facilitate running the test suite, a configuration file is present
in ``drivers/gpu/drm/tests/.kunitconfig``. It can be used by ``kunit.py`` as
follows:

.. code-block:: bash

	$ ./tools/testing/kunit/kunit.py run --kunitconfig=drivers/gpu/drm/tests \
		--kconfig_add CONFIG_VIRTIO_UML=y \
		--kconfig_add CONFIG_UML_PCI_OVER_VIRTIO=y

.. note::
	The configuration included in ``.kunitconfig`` should be as generic as
	possible.
	``CONFIG_VIRTIO_UML`` and ``CONFIG_UML_PCI_OVER_VIRTIO`` are not
	included in it because they are only required for User Mode Linux.

KUnit Coverage Rules
~~~~~~~~~~~~~~~~~~~~

KUnit support is gradually added to the DRM framework and helpers. There's no
general requirement for the framework and helpers to have KUnit tests at the
moment. However, patches that are affecting a function or helper already
covered by KUnit tests must provide tests if the change calls for one.

Legacy Support Code
===================

The section very briefly covers some of the old legacy support code
which is only used by old DRM drivers which have done a so-called
shadow-attach to the underlying device instead of registering as a real
driver. This also includes some of the old generic buffer management and
command submission code. Do not use any of this in new and modern
drivers.

Legacy Suspend/Resume
---------------------

The DRM core provides some suspend/resume code, but drivers wanting full
suspend/resume support should provide save() and restore() functions.
These are called at suspend, hibernate, or resume time, and should
perform any state save or restore required by your device across suspend
or hibernate states.

int (\*suspend) (struct drm_device \*, pm_message_t state); int
(\*resume) (struct drm_device \*);
Those are legacy suspend and resume methods which *only* work with the
legacy shadow-attach driver registration functions. New driver should
use the power management interface provided by their bus type (usually
through the :c:type:`struct device_driver <device_driver>`
dev_pm_ops) and set these methods to NULL.

Legacy DMA Services
-------------------

This should cover how DMA mapping etc. is supported by the core. These
functions are deprecated and should not be used.
