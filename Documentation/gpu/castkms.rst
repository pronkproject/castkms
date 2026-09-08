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

Those checks establish ordinary DRM submission behavior, not presentation
timing, GPU interoperability, capture authorization, or casting performance.

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
