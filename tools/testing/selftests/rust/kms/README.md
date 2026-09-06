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
virtual display with no physical panel and no compositor, then runs ownership,
modesetting, page-flip completion, allocation failure, and registration
teardown for real.

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
framework's supported path or from the framework's explicit "this CRTC has no
vblank" path. A driver cannot claim vblank support on a CRTC whose vblank
array was never initialized.

The per-file and memory-object types used by the device must name that same
driver as their owner. Memory objects here are GEM buffers. GEM, or Graphics
Execution Manager, provides shared machinery for managing graphics storage.
The tests substitute a provider from another driver into an otherwise valid
driver type and expect a compile error.

Shared and exclusive views of unpublished atomic state cannot coexist. If one
piece of code is allowed to change the next state, another cannot also hold a
shared view of that same next state, including when the second attempt tries
the reverse lock order or re-enters an iterator. After a commit, event
handling may still look at old and new CRTC state, but it may not change the
driver's private data through that path.

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

Holding driver-private data does not, by itself, keep the parent device bound
to its driver or keep resources released during unbind available.
The constructor that lets a registration escape its setup function is unsafe
for the same reason the borrowed constructor is unsafe: the caller must finish
unplug, meaning the device is no longer visible and its users have drained,
before releasing parent resources that the registration still refers to.
Installing a managed cleanup action is not proof of that ordering for
resources added afterwards. The compiler rejects calling that escaping
constructor outside an unsafe block. In the API it is
`Registration::new_static()`.

The safe alternative lends registration to a callback while the parent is
still bound, and the framework owns teardown. Returning a registration handle
from that callback is a compile error. There is no safe long-lived owner that
can escape the callback. In the API that constructor is
`Registration::with_static()`.

Kernel-initiated atomic updates, the kind that do not come from a userspace
ioctl (a device request made through an open file), also require the live
registered view. The callback may borrow a
transaction, but it cannot keep a handle to the next state after the callback
returns. Programming the primary plane and connector routing needs exclusive
access to the helper that edits that configuration, so outstanding state
handles cannot race it. That configuration helper cannot be sent to another
task even when the driver type itself can. A shareable device reference is
not a substitute.

The same callback and registration rules apply to a validation-only request,
which is an atomic check that must not change published state. The compiler
rejects keeping its state handle after the callback, and rejects calling it
through a merely allocated device.

Display timing for vblank waits is read from the commit's CRTC state. The
vblank lock does not protect the mode fields that native modesetting writes,
so reading a cached mode or cached duration through that lock is rejected. A
timer driver has to copy validated timings into storage it actually
synchronizes.

These checks are about function signatures and what the compiler will accept.
They do not prove every rule that an `unsafe` block is still required to
uphold. They do not test runtime device identity, allocator failure, reset,
page-flip events, suspend, or GPU execution.

## Runtime checks

The in-kernel suite lives in `rust/kernel/drm/kms/tests.rs`. Most cases build
an unregistered virtual display: a fake bus device that never publishes a DRM
character device, so userspace never sees a `/dev/dri/card*`. A few
registration cases deliberately publish a temporary virtual card. Run those in
a virtual machine or a dedicated test boot, not on the desktop you are working
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

### Atomic updates without a real compositor

The next question is whether an update can fail, retry, and refuse to mix
objects from the wrong place, without a userspace compositor in the loop.

A rejected update copies CRTC state, aborts, and retries with a new
transaction. The error must propagate, the already published state must stay
put, and the failed transaction's locks must be released.

Two tasks can take CRTC and plane locks in opposite orders. DRM's deadlock
protocol requires an actual "deadlock, please backoff" error, then a retry
with fresh private state. Validation must leave published state unchanged.
Commit publishes only the final successful attempt. Another case checks that
retry still happens if the driver's callback catches the deadlock error.
A separate case returns the same error value without a real lock conflict
and requires the helper not to retry blindly. These are two-task schedules,
not a proof of every lock order
and not an allocator-stress test.

A plane check must not accept a CRTC from another output, and it must not
accept CRTC state from another transaction. The first case adds a second CRTC,
rejects the mismatch without changing the plane's computed position, then
accepts the assigned CRTC. The second allocates two transactions on one task,
never commits the extra one, and rejects the mixed pair. Both use real DRM
allocations. They do not fabricate object pointers.

A validation-only request can check a complete modeset without changing
published state and without calling enable, disable, or plane-update. A later
transaction then proves that the check released its resources. Success here is
not a reservation that a later commit will succeed.

### Selecting an image for a virtual output

The fake monitor must select an image, replace it, and agree with the kernel
about the mode timings. Those checks inspect display state; they do not render
or visually compare pixels.

The primary modeset case allocates a GEM framebuffer, meaning a pixel buffer
backed by the kernel's GPU buffer objects, programs a 640x480 image for the
controller to send to the fake display, observes that the published
framebuffer is the one just set, and disables the output before teardown.
Enable and disable callback counts show that the helper drove the CRTC's
software callbacks. The test driver runs the
Rust work that happens after the update is accepted, including a fake vblank.

A constructed mode is checked for derived clock, total lines, and blanking
interval. Kernel-created modes use the same timing initializer that userspace
modes go through, so the two paths should agree.

Replacing a framebuffer without changing the mode drops the client's first
buffer after the output is enabled, then submits a second image. The selected
image must change, plane-update must run, a full modeset must not, and the
retired buffer must disappear once disable releases the last display
reference. This path completes immediately through the fake vblank. It is not
a delayed display event, and it is not a test of a GPU still reading the old
buffer.

### Page flips and vblank

Showing a new image is not the same as the old image becoming free. The old
framebuffer has to stay until the fake display has actually passed the point
where it would have scanned the new one.

A second suite, `rust_drm_events`, uses a virtual driver with real pending
flip-completion events and initialized vblank storage. It omits the optional
Rust plane-update method on purpose, so the framework's do-nothing callback is
used instead of a null C function pointer.

Enable and disable send their completion events immediately. Replacing the
image arms an event for a later vblank. The work after the update is accepted
reports that hardware programming finished and returns without waiting for the
flip. A second task blocks in that replacement. After programming completes,
the test waits 20 milliseconds, checks that the replacement is still pending
and that both framebuffers still exist, then drives vblank. Completion must
retire the old image. Disable must release the remaining one.

Turning vblank off instead of delivering the interrupt must drain the armed
event and reject a later fake interrupt. As a negative control, removing the
framework's automatic wait for flip completion makes both cases fail their
"still pending" check. The driver callback omits an explicit wait, so the
framework must supply it before releasing the old state.

These events are internal completion objects. They are not the page-flip
events userspace reads from a DRM file. The schedules do not claim to cover
arbitrary interrupt races, a GPU still reading the old buffer, concurrent
unplug, or delivery to a DRM file.

An owned vblank reference can outlive the borrowed device handle used to
create it. Converting a borrow into an owned handle must not change the native
count; dropping the owned handle must release exactly one reference. That is
allocation lifetime, not a claim that the parent CRTC remains usable for
hardware access.

### Private state, properties, and real allocation failure

Drivers hang private data off CRTC, plane, and connector state. That
allocation can fail, and a failed setup must not leave a half-attached
property or a published copy that was never meant to be visible.

The tests cover three transitions for each object: construction failure,
duplication failure, and a successful copy that can be changed without
changing the published original. They retry after failure and check that live
private-data counts return to zero. Connector copies are read through the
shared typed wrapper while only the unpublished copy is changed. Failure is
injected at the private-data hook. It is not, in these cases, a fault in the
kernel's object allocator. The native duplicate-state callback can only
report failure as a null pointer, which becomes "out of memory."

### Registering and unregistering the virtual card

Publishing a `/dev/dri/card*` is the point at which the fake monitor becomes
visible. Taking it back must turn the output off and refuse new users,
including when another thread is still holding a handle.

The public constructor that registers for the rest of the device's life can
obtain a registered-device handle and run validation through the public API.
After registration is dropped, a leftover device reference must not be able to
obtain a new handle. The remaining mode objects disappear when that leftover
reference is dropped.

One case commits an image through that public handle, then drops registration
without first disabling the CRTC. Teardown must turn the output off, release
the framebuffer, and reject later handles. This does not cover concurrent ioctl
users or delayed page-flip events.

The scoped constructor is the same story with a callback. The device
allocation can outlive the callback after success and after a cancelled check.
In both cases, new registration handles must be rejected, and dropping the
allocation must release the mode objects.

Concurrent teardown moves registration and its fake parent to a background
job, dropping them in that order. A handshake holds teardown until the test
has a registration handle. New handles are rejected while teardown is blocked.
The existing handle can still validate an update. Releasing it lets teardown
finish. The background job is allocated before the handle is taken, so a
failed spawn cannot wait on the caller. This proves the kernel waits for
current users before the device is destroyed. It does not prove arbitrary
parent-resource ordering or concurrent userspace ioctls.

Object destruction counters are not a leak detector. Partial setup rejection
is not allocator fault injection. Delayed GPU-reader retirement, suspend,
userspace unbind stress, and real GPU execution remain separate work.
