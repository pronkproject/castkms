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

One Vulkan device allocates eight images (256-by-256 and linear by default)
and fills them using GPU commands, with a different opaque RGB color for each image. Another Vulkan
device imports those allocations through DMA-BUF descriptors. The producer
releases external image ownership, submits its work, and exports a sync-file
semaphore for each image. The consumer imports that
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

The three devices live for the whole batch. All eight source-reading jobs are
submitted before the first host completion wait; all eight sources are then
released before any output submission. Similarly, every output job is
submitted before waiting for output completion. There is no explicit host wait
between successive jobs within a stage. The fixture reports those boundaries
and checks every individual native completion and every frame's pixels.
Eight is the size of this test batch, not a kernel queue limit or media policy.
The small jobs may finish quickly, so the test does not establish a measured
number of simultaneously executing GPU jobs.

Before releasing A, the fixture also polls its exported completion and queries
Linux's sync-file status, including the underlying native fences. A signaled
error fails the test; readability alone is not successful image production.
Vulkan is also allowed to return an already-completed sentinel instead of a
descriptor. The fixture reports that case explicitly. A timeout is a test
failure, not cancellation: cleanup still waits for submitted GPU work.

`"$build_dir/sync-file-test"` checks rejection of invalid descriptors,
unreadable pipes and readable objects that are not sync files. It requires no
GPU and intentionally prints diagnostics for the rejected cases.

The default modifier is linear (zero). Pass `--modifier INTEGER` to exercise
another modifier reported by the probe, for example a hexadecimal value
beginning with `0x`. The fixture requests exactly that modifier for A, E and D;
it never silently substitutes another layout. Unsupported profiles fail
before image creation. Each import uses the allocation's queried plane layout,
not a pitch inferred from its width. Qualification of one modifier does not
establish that a later encoder or PipeWire consumer can import it.

Pass `--size 1920x1080` or `--size 3840x2160` to run the same allocation,
native-completion and whole-image checks at display sizes. `--size 256x256`
selects the default explicitly. These fixed profiles bound the fixture's
allocation demand; other sizes, incomplete values and duplicate options are
rejected before opening the GPU. They are test inputs, not supported display
modes or kernel limits.

The batch allocates eight independent A, E, D and readback images before
submission. At 4K their unpadded pixel storage alone is about 1 GiB; modifier
padding and driver bookkeeping add to that. A successful large-image run
checks import layout and pixels, not sustained throughput, staging bandwidth
or a frame-rate target. There is still a host wait between the source and
output stages, and the output stage includes the test-only readback copy.

Run `sh handoff-args.sh "$build_dir/handoff"` from this directory to check
rejection of malformed and repeated size options without a GPU. Those checks
verify the usage error, not merely a failure to open the supplied device.

Add `--timing` to record a pair of GPU timestamps for each source and output
job. The source interval covers imported-image acquisition, A-to-E blitting
and E's external ownership release. The output interval covers acquisition
and E-to-D blitting, ending before the oracle copy and D's final ownership
release. An execution barrier keeps the oracle copy after that timestamp.
Query results are read only after the containing submission completes. No
timestamp is subtracted from a timestamp on a different Vulkan device.

The report includes raw ticks, the selected queue's valid counter width,
nanoseconds per tick and the resulting milliseconds. A queue without usable
timestamps fails an explicitly requested timing run; ordinary correctness
runs do not need timestamp support. Subtraction handles a counter wrapping
through zero, and a conservative host-lifetime bound rejects intervals long
enough to make the wrap count ambiguous. See Khronos's contracts for
[timestamp writes](https://docs.vulkan.org/refpages/latest/refpages/source/vkCmdWriteTimestamp.html)
and [query results](https://docs.vulkan.org/refpages/latest/refpages/source/vkGetQueryPoolResults.html).

These are instrumented command intervals, not isolated memory bandwidth or
end-to-end latency. They include barriers and may reflect contention or work
overlap on the shared GPU. Instrumentation also changes scheduling. Record
validation settings and compare repeated runs; do not turn eight samples into
a sustained-cadence claim or sum intervals that might overlap. Allocation,
host scheduling, media conversion, encoding and receiver presentation are
outside the measured stages.

The output submission exports its own native sync file, separate from the
A-to-E completion. The fixture checks that result too. D is released in the
general image layout for external use. Add `--foreign-output` when testing
release to a different driver or API, such as VA. That explicitly enables
Vulkan's foreign-queue extension and uses its ownership-transfer boundary;
ordinary external Vulkan queues assume the same driver. See Khronos's
[foreign queue contract](https://docs.vulkan.org/refpages/latest/refpages/source/VK_EXT_queue_family_foreign.html).

The fixture does not send D to a media process. A successful foreign release
and native completion are necessary inputs to that experiment, not evidence
that the other driver imported the allocation or produced an encoded frame.

## Persistent pools under consumer backpressure

The separate `reuse` executable checks persistent allocation reuse:

```sh
VK_VALIDATION_VALIDATE_SYNC=1 "$build_dir/reuse" /dev/dri/renderD128 --validation
```

Add `--modifier INTEGER` to select a reported modifier. The image size is
fixed at 256-by-256. Two staging allocations E are shared between a source
worker and an output worker; three output allocations D are shared between
the output worker and a simulated consumer. Each worker has its own Vulkan
device. Allocations and imported images persist for four rounds, serving
twenty changing source images. Pool sizes are fixture budgets, not kernel
limits or a prescribed relationship to a transport window.

Each round fills D and leaves its ownership with the consumer. While all
three outputs remain held, two more generated sources are cleared and blitted
into available E allocations. Each source is destroyed after its submitted
native completion is checked, without returning any D. Once E is full, another
capture attempt returns backpressure before allocating or accessing a source.
The consumer reads every held D again into fresh oracle storage and checks
that the pixels still match its earlier frame. Returning D permits the queued
E images to drain; the following round reuses the same imports with new pixels.

E returns from the output worker before the source worker writes it again.
D returns from the consumer before the output worker writes it again.
Both directions use external queue ownership barriers in the general layout
and sync-file semaphores exported from accepted submissions. An E-to-D job's
completion covers both its E read and D write, but that job is admitted only
when D is new or has been returned. Its completion is not part of the preceding
source image's retirement. See Khronos's
[external queue ownership rules](https://docs.vulkan.org/spec/latest/chapters/synchronization.html#synchronization-queue-transfers).

`recycle.c` handles the native image exchanges, submission lifetimes and pixel
oracle. `reuse.c` owns the separate staging/output budgets and admission policy.
Command buffers, semaphores and oracle storage are reclaimed after each
operation; the native command pool does not grow with frame count. Any native
error aborts the experiment. A timeout is not cancellation: teardown still
waits for accepted commands and is not a bounded GPU-hang recovery mechanism.

Run `"$build_dir/reuse-test"` for a hardware-independent policy check, failure
injection at every operation boundary, partial-construction teardown checks
and malformed-argument rejection. Its doubles reject overwrite of occupied
staging or output slots and verify delivered frame identities. Those checks
do not validate the Vulkan implementation, native allocation-failure paths or
real pixels; the GPU executable provides the separate native exercise.

The consumer hold is an application-level retention of a real imported GPU
image, not an unsignaled encoder fence or simulated GPU hang. Operations wait
for completion on the host to make the ordering deterministic. There is no
throughput claim, overlapping GPU-work qualification, PipeWire release
protocol, hardware encoder or KMS transaction here. Generated A is local to
the source worker; the separate `handoff` fixture exercises producer-device
import. Readback is solely the test oracle, not the proposed media path.
An installed media consumer's returned-buffer synchronization and a submitted
downstream reuse wait remain separate qualifications.

All submitted uses finish before their resources are destroyed, including on
test failure. Import helpers track whether Vulkan consumed each descriptor,
including failures after memory import; cleanup closes only descriptors still
owned by the caller. Image allocation, import and device
setup live separately from the test's submission sequence. The fixture uses
only its own generated content, not compositor or CastKMS source buffers.
