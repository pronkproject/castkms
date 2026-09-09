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
