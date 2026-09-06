# Compiler and runtime checks for the Rust display API

CastKMS wants a virtual monitor whose kernel side is written in Rust. The
kernel should own display state, timing,
and access policy. Pixel composition should move to a trusted userspace
program that talks to a graphics processing unit (GPU) through ordinary
buffer sharing and completion signals, called fences. Before
that virtual driver can authorize capture or wait for userspace rendering, the
shared Rust display types have to mean what they say.

KMS is Kernel Mode Setting, the part of DRM that describes a display pipeline.
DRM is Direct Rendering Manager, the kernel subsystem for GPUs and display
controllers. A CRTC is the display controller that scans out an image. A plane
is a layer of pixels on that controller, such as the primary fullscreen image
or a cursor. A connector is the logical plug, such as a virtual HDMI output. An
encoder sits between the controller and the connector. A framebuffer describes
an image's layout and backing pixel storage; it need not currently be shown.
Vblank is the interval between successive scans of the display. Drivers use
it to track presentation timing, and ordinary synchronized page flips change
the selected framebuffer at that boundary. An atomic update validates and
applies a configuration involving several display objects as one transaction,
rather than requiring userspace to program each object separately.

The current Rust wrappers for that pipeline are early. These tests exist to
catch the kinds of mistakes that are cheap to make in a typed wrapper around
C: mixing one driver's objects with another, mutating state that has already
been published, carrying a commit step out of the function that is allowed to
take it, or treating an allocated-but-not-yet-visible device as if userspace
could already see it.

There are two kinds of tests. They answer different questions.

The compiler suite never loads a driver and never talks to hardware. It asks
the Rust compiler, against the compiled Rust kernel library you just built,
whether legal driver code still type-checks and whether a deliberately illegal
example is rejected for the documented reason.

The runtime suite boots inside a disposable test kernel. It builds a fake
virtual display with no physical panel and no compositor, then checks
object ownership and setup failure for real.

Passing either suite does not mean a virtual KMS driver is ready to capture
frames. It means the shared types are less likely to lie before that driver is
written.

## Compiler checks

Each file in this `kms` directory is test data, not a userspace program. The
file contains a legal default example. Building the same file with
`--cfg negative` switches in an illegal example. A test passes only when all
three of these are true:

- The legal example compiles.
- The illegal example fails to compile.
- The compiler diagnostic matches the `error-pattern` comment in that file.

If the compiler fails for some other reason, such as a mismatched toolchain or
a missing kernel library, that is not evidence that the API rejected the
illegal operation. It is just a broken test environment.

### How to run them

Build an x86-64 kernel with Rust and DRM enabled, using the kernel's documented
Rust toolchain. After configuring an out-of-tree build:

```sh
make O=/path/to/build LLVM=1 rustavailable
make O=/path/to/build LLVM=1 -j2 rust/kernel.o
RUSTC=/path/to/the/same/rustc ./tools/testing/selftests/rust/kms-typecheck.sh /path/to/build
```

The compiler used to build the kernel and the compiler used to run this script
must be the same. With rustup, set `RUSTUP_TOOLCHAIN` to that toolchain for
both steps. Do not mix metadata produced by different compiler versions.

If you omit the build-directory argument, the script accepts `KBUILD_OUTPUT`.
If the build is missing, is not x86-64, or lacks `CONFIG_RUST=y` and
`CONFIG_DRM=y`, the script reports a kernel-selftest skip rather than a
failure. Compiler logs are kept in a temporary directory printed in the TAP
output, which is the simple "ok / not ok" format those selftests use.

The suite can be installed through the Rust selftest Makefile. Keep this `kms`
directory next to `kms-typecheck.sh` after installation.

### What they protect

A driver names one concrete type for each kind of display object. Building a
CRTC must use that driver's CRTC, not some other implementation that happens
to share a driver interface. The same rule applies to planes, connectors, and
encoders. The private state hung off each object must point back at the same
object type. Turning a generic CRTC handle into the driver's own mutable view
follows the same rule: a matching type compiles, another driver's type does
not. The compiler, not a runtime panic, is what rejects a mismatch.

The C callback tables generated for those objects are not interchangeable. A
plane implementation cannot borrow another plane's table, and likewise for
CRTCs, connectors, and encoders. Vblank callbacks come only from the
framework's supported path or from the framework's explicit disabled path.

The per-file and memory-object types used by the device must name that same
driver as their owner. Memory objects here are GEM buffers. GEM, or Graphics
Execution Manager, provides shared machinery for managing graphics storage.
The tests substitute a provider from another driver into an otherwise valid
driver type and expect a compile error.

Shared and exclusive views of unpublished atomic state cannot coexist. If one
piece of code is allowed to change the next state, another cannot also hold a
shared view of that same next state, including when the second attempt tries
the reverse lock order or re-enters an iterator.

An atomic commit walks through named steps, such as programming the mode and
then updating planes. Proof that a step happened cannot be returned from the
callback, handed to another transaction, handed to another driver, reused
after it was consumed, or used to skip a required step. Both step orderings
that the framework actually supports still compile.

Device lifetime is staged on purpose. An allocated DRM device is not yet an
initialized KMS setup, and an initialized setup is not yet a device visible to
userspace. Locking the mode configuration requires one of the later views.
Reading the CRTC count requires either the single-threaded setup view or a
completed registration, so a device pointer that escaped during setup cannot
be used for a racing count read. Hotplug events, the "a monitor appeared or
disappeared" notifications, require the live registration view. The driver
interface also has to select KMS setup on the registration path; a driver
cannot claim to be a KMS driver and then skip that step.

A general device handle still does not prove that the device is registered.
The registration object has a separate handle for that. The compiler tests
check the distinction without actually registering a DRM device. In the API
those two methods are `Registration::device()` and
`Registration::registration_guard()`.

These checks are about function signatures and what the compiler will accept.
They do not prove every rule that an `unsafe` block is still required to
uphold. They do not test runtime device identity, allocator failure, reset,
page-flip events, suspend, or GPU execution.

## Runtime checks

The in-kernel suite lives in `rust/kernel/drm/kms/tests.rs`. Its cases build
an unregistered virtual display: a fake bus device that never publishes a DRM
character device, so userspace never sees a `/dev/dri/card*`. Run those in a
virtual machine or a dedicated test boot, not on the desktop you are working
on. No case talks to a physical panel, a capture pipeline, or a compositor.

### How to run them

KUnit is the kernel's in-process unit test framework. Build and boot a
disposable test kernel with KUnit, Rust DRM, and the shared-memory GEM helper
that the test driver consumes. That helper is selected by the consuming
driver. Check that it appears in the generated configuration; setting a hidden
option by hand may not survive.

On the kernel command line, select `kunit.filter_glob=rust_drm_kms` for the
main suite, or `rust_drm*` to include the existing shared-memory and
framebuffer helper tests as well. Read the KTAP results, which is KUnit's
"ok / not ok" output. A boot that selected zero tests is not a pass.

There is a separate C companion, the existing `drm_crtc` KUnit suite. Enable
`CONFIG_DRM_KUNIT_TEST=y` and select `kunit.filter_glob=drm_crtc`. It stubs a
test hook that would checksum scanned-out pixels so initialization returns
"out of memory," checks that the rejected CRTC did not leak into the device
list, count, or plane mask, and retries on the same device. It does not
exercise the allocator itself.

### Building a fake display

Before anyone modesets, the fake pipeline has to come up as a consistent set
of objects and go away again without leaking them.

The basic cases check that each plane, CRTC, and connector's initial state
points at the object that owns it, that destroying those objects drops the
expected counts, and that a deliberately rejected partial setup unwinds. They
also reject attaching a primary or cursor plane that belongs to a different
device.

Names that contain a percent sign are copied as literal text. They must not be
interpreted as `printf` format strings, which could make a display name trigger
invalid memory access. That is checked independently for planes,
CRTCs, and encoders.

Once KMS setup has finished, the mode-configuration mutex can be acquired,
dropped, and acquired again, and it reports the owning device. The tests do
not call the operations that are illegal before initialization. Those are
rejected by the compiler suite instead of being executed as undefined behavior
in the virtual machine.

Object destruction counters are not a leak detector. Partial setup rejection
is not allocator fault injection. Delayed GPU-reader retirement, suspend,
userspace unbind stress, and real GPU execution remain separate work.
