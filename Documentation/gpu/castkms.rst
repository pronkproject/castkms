.. SPDX-License-Identifier: GPL-2.0-only

CastKMS virtual display
======================

CastKMS is being developed as a virtual display whose images will eventually
be composed in userspace. The Rust driver currently provides only the display
device on which that work will build. It is not a replacement for a working
C CastKMS casting installation.

Enable ``CONFIG_DRM_CASTKMS`` in a kernel with Rust support to create one
always-connected virtual output. The driver accepts atomic modesetting and
linear XRGB8888 framebuffers, with local storage allocated through the usual
DRM dumb-buffer interface. Modes up to 1920 by 1080 are offered for development.
That size is a temporary driver limit, not a receiver or transport policy.
Foreign DMA-BUF import is not enabled yet.

There is no display clock. A successful flip event means that the driver has
accepted the new state and no longer needs the old buffer; it does not mean
that a frame has been displayed elsewhere. Events complete through DRM's
existing mechanism for devices without vertical blanking interrupts. The
driver does not read framebuffer pixels, retain them for capture, or implement
any private capture requests. Normal DRM operations on a caller's own buffers
are not a capture capability.

There is no cursor plane, configurable display attachment, EDID, audio, CEC,
writeback, CRC collection, or delegated composition. No default framebuffer
console client is started. Keep production casting on the existing driver
until the required facilities have been implemented and qualified.

Code boundaries
---------------

``castkms.rs`` owns the virtual parent device and DRM registration. Destruction
unplugs DRM and shuts down atomic state before releasing the parent. Display
objects may remain allocated while existing DRM references are being released;
their data does not borrow the module's registration storage.

``display.rs`` defines the output and its atomic checks. ``gem.rs`` defines the
private buffer payload and opts into local allocation. Neither layer calls a
capture file adapter. Shared DRM helpers remain responsible for object
allocation, framebuffer validation, atomic transaction locking and ordinary
ioctl handling. Future capture policy must not turn buffer allocation or the
ordinary commit path into an implicit grant of access to another client's
pixels.

``scene.rs`` retains the primary plane's framebuffer allocation and copied
source and destination geometry independently of the atomic callback. The
source coordinates keep their original fixed-point representation. Atomic
validation prepares geometry in the candidate plane state; the plane update
callback replaces the output's description before flip completion. Test-only
and rejected transactions do not publish a replacement. An inactive or
disabled plane clears the description. Recommitting the same framebuffer
still replaces the description; framebuffer identity is not a content cache.

Each checked plane update that publishes a scene derives a content serial
from that plane's last accepted atomic state. The serial is installed only
if the update is accepted; test-only submissions and failed candidates do
not consume numbers. Rechecking one candidate derives the same successor
rather than incrementing it again.
Blank updates preserve the counter, so reactivation continues the sequence.
Exhaustion rejects candidates that publish a scene with ``EOVERFLOW`` rather
than reusing a serial; blanking remains possible. The serial is internal and
meaningful only within one plane lifetime.

The serial conservatively advances for every accepted scene update, even when
the framebuffer and geometry are unchanged. It marks a possible content change,
not proof that pixels differ or that rendering succeeded. It does not identify
the framebuffer's creator, adopt a new capture owner, or replace authorization.

The output holds at most one description. Replacement releases the previous
reference outside the output lock. Module teardown permanently closes the
output before releasing DRM registration, so an outstanding commit cannot
restore its framebuffer reference after shutdown begins. That ordering breaks
the reference cycle between the output, framebuffer and DRM device.

An owned allocation does not preserve its pixels against later writes. The
description retains historical ownership resolved during atomic validation,
but that attribution is not current permission to capture. Each update retains
its explicit plane producer fence before DRM's commit helper waits and drops
that fence. A producer error remains observable after an otherwise accepted
display commit; completion alone does not establish valid pixels. A new update
does not inherit the previous update's fence, including on a same-framebuffer
recommit. An absent fence is missing synchronization evidence, not a success
result.

Required implicit dependencies and a read lease are still absent. There is no
interface for reading or exporting pixels. Capture publication must establish
permission and those remaining lifetime checks before using descriptions for
deferred work. A later reservation scan cannot recover producer error history
that was discarded before collection.

Testing in a disposable virtual machine
--------------------------------------

The userspace smoke test requires the libdrm development headers and library::

    make -C tools/testing/selftests/drm_castkms
    tools/testing/selftests/drm_castkms/modeset /dev/dri/cardN

Choose the Rust CastKMS node explicitly in an otherwise unused test VM. The
test changes display state and requires DRM master access. Do not run it
against an active desktop. Without a node argument it skips instead of
selecting a device automatically. It checks the driver name and development
version before attempting a modeset.

The test allocates and maps two local buffers, verifies that a test-only
commit leaves the display inactive, enables the output, and submits 48 flips
including same-framebuffer updates. Each submitted flip must produce exactly
one event. A scaling request must be rejected. Finally the test disables the
output and releases its framebuffers, buffer handles and mode description.

The test also submits a test-only framebuffer replacement while the output is
active and verifies that the selected framebuffer remains unchanged. It turns
the output off and on without removing its plane configuration before testing
further flips. Optional ``CONFIG_DRM_CASTKMS_KUNIT_TEST`` tests exercise the
output's resource replacement, blanking and terminal shutdown rules, including
resource destruction that reenters output shutdown. Those tests do not read
framebuffer pixels or establish capture authorization.

Those checks establish ordinary DRM submission behavior, not presentation
timing, GPU interoperability, capture authorization, or casting performance.

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
Neither userspace test adds a private ioctl or exports pixels.

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
tests do not implement implicit dependency collection or deferred source reads.

Building without another display driver
--------------------------------------

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
