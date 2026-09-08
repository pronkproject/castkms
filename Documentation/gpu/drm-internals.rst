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
kernel-controlled snapshot publication. That is a reference-provider first
cut, not a VKMS integration or a public capture interface. Capture-holder
files, DRM grant policy, destination DMA-BUF registration,
format negotiation, provider notification and safe Rust ownership wrappers
are subsequent integrations. In particular, the CPU snapshot helper does not
claim the delegated GPU composition path or create a future-userspace fence.

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
