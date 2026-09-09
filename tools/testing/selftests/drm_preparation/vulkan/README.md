# Stock Vulkan investigation

The preparation models and kernel tests do not establish that the intended
graphics stack can import an image or export its real GPU completion. This
opt-in directory investigates that separate boundary using Vulkan. It does
not open a DRM primary node itself, inspect desktop framebuffers, or deploy
the experimental preparation kernel on the running system.

Build with a C compiler, Vulkan headers and the Vulkan loader development
package. The parent directory's Python model tests do not require those
dependencies.

```sh
build_dir=$(mktemp -d)
make -C tools/testing/selftests/drm_preparation/vulkan OUTPUT="$build_dir"
"$build_dir/probe" /dev/dri/renderD128
```

Pass the render node you intend to test. Its filename is not persistent GPU
identity: the program matches the node's device number against Vulkan's DRM
device properties, then prints the selected GPU and driver. It does not silently
choose a software renderer or another physical device. Exit status 4 means the
selected device or required capabilities are unavailable; 1 is a failure.

An empty Vulkan device list is unavailable hardware. A failed enumeration is
instead a test failure, with the Vulkan error printed for diagnosis. Run
`"$build_dir/context-test"` to exercise those distinctions without a GPU. It
substitutes the instance and enumeration calls, checks both stages of device
discovery, and verifies that each created instance is destroyed exactly once.
The expected failure cases intentionally print diagnostic messages.

The probe checks sync-file semaphore import/export and the DMA-BUF image
capabilities of B8G8R8A8_UNORM, one opaque fullscreen RGB investigation format.
It reports every advertised DRM modifier and whether it supports the initial
single-memory-plane blit profile. A reported candidate is not proof that image
creation, transfer, synchronization or encoding has worked.

With the Khronos validation layer available, add `--validation`. A requested
layer that cannot load is an error, not a silent unvalidated run. Validation
errors also fail the program. The optional loader environment variables
`VK_DRIVER_FILES` and `VK_LAYER_PATH` can select an explicitly recorded driver
and separately unpacked validation layer for an investigation; they do not
change the installed graphics stack.

Record the running kernel and configuration, GPU identity, loaded firmware,
Mesa/driver and loader versions alongside results. Media and service-sandbox
qualification are separate experiments. In particular, a capability query is
neither a throughput measurement nor a demonstration of the complete CastKMS
capture path.

The queries follow Khronos's
[external image format contract](https://docs.vulkan.org/refpages/latest/refpages/source/VkPhysicalDeviceExternalImageFormatInfo.html).
Any later image import must also satisfy the
[explicit modifier layout contract](https://docs.vulkan.org/refpages/latest/refpages/source/VkImageDrmFormatModifierExplicitCreateInfoEXT.html),
not merely reuse a modifier number reported by the probe.

## Generated-image handoff

The `handoff` executable tests a small, real transfer on the selected device:

```sh
"$build_dir/handoff" /dev/dri/renderD128 --validation
```

For an additional synchronization-validation pass, use
`VK_VALIDATION_VALIDATE_SYNC=1` with the same command. That enables the
Khronos layer's checks for missing dependencies between graphics operations.
Those checks have limits, including memory-alias tracking; passing them does
not prove all ordering across imported allocations. See the layer's
[synchronization validation guide](https://vulkan.lunarg.com/doc/view/latest/linux/synchronization_usage.html).

One Vulkan device allocates a 256-by-256 linear image and fills it using a GPU
command. The fixture repeats the sequence for eight opaque RGB colors, with
fresh devices and allocations each time. Another Vulkan device imports the
same allocation through a DMA-BUF descriptor. The producer releases external image ownership,
submits its work, and exports a sync-file semaphore. The consumer imports that
semaphore and waits on it before accessing the image. There is no host-created
promise standing in for work that has not been submitted.

That first consumer is the source worker. It blits A into an independently
allocated private image E and exports the submitted operation's sync file.
After the producer and source worker finish, the fixture destroys both Vulkan
images referring to A. Only then does a third Vulkan device submit a blit from
E into a separately allocated, exportable output D. No allocation belonging
to A participates in that final submission.

The output worker copies D to a host-visible buffer solely to check every
pixel. That final test readback is not a proposed capture transport. The
fixture has not yet exercised downstream backpressure, passed a frame through
PipeWire or an encoder, or integrated with a CastKMS preparation transaction.
It is a correctness test, not a throughput measurement. Waiting for source
completion on the host deliberately makes the destruction-before-output
ordering visible; it is not a proposed production scheduling policy.
Changing colors exercises actual output contents across repeated handoffs,
but does not qualify reuse of a persistent image pool.

Before releasing A, the fixture also polls its exported completion and queries
Linux's sync-file status, including the underlying native fences. A signaled
error fails the test; readability alone is not successful image production.
Vulkan is also allowed to return an already-completed sentinel instead of a
descriptor. The fixture reports that case explicitly. A timeout is a test
failure, not cancellation: cleanup still waits for submitted GPU work.

`"$build_dir/sync-file-test"` checks rejection of invalid descriptors,
unreadable pipes and readable objects that are not sync files. It requires no
GPU and intentionally prints diagnostics for the rejected cases.

All submitted uses finish before their resources are destroyed, including on
test failure. Successful Vulkan imports consume their descriptors; failed
imports leave the descriptors for cleanup. Image allocation, import and device
setup live separately from the test's submission sequence. The fixture uses
only its own generated content, not compositor or CastKMS source buffers.
