# Compiler checks for the Rust display API

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

The compiler suite never loads a driver and never talks to hardware. It asks
the Rust compiler, against the compiled Rust kernel library you just built,
whether legal driver code still type-checks and whether a deliberately illegal
example is rejected for the documented reason.

Passing the compiler suite does not mean a virtual KMS driver is ready to
capture frames. It means the shared types are less likely to lie before
that driver is written.

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

Shared and exclusive views of unpublished atomic state cannot coexist. If one
piece of code is allowed to change the next state, another cannot also hold a
shared view of that same next state, including when the second attempt tries
the reverse lock order or re-enters an iterator.

An atomic commit walks through named steps, such as programming the mode and
then updating planes. Proof that a step happened cannot be returned from the
callback, handed to another transaction, handed to another driver, reused
after it was consumed, or used to skip a required step. Both step orderings
that the framework actually supports still compile.

These checks are about function signatures and what the compiler will accept.
They do not prove every rule that an `unsafe` block is still required to
uphold. They do not test runtime device identity, allocator failure, reset,
page-flip events, suspend, or GPU execution.
