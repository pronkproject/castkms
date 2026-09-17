==================
DRM capture grants
==================

The experimental capture interface separates permission to receive a final
output image from modesetting, source-buffer access and rendering. Its current
public operations create a grant, describe an offered image configuration,
create or destroy streams, register or remove destination storage, and queue,
cancel or dequeue final-image output. The interface assignments are development
ABI, not upstream allocations.

Issuing a grant
==============

``DRM_CAP_CAPTURE_GRANT`` reports support for creator-bound grant issuance on
a KMS device. A current master file supplies the CRTC and connector IDs to
``DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT``. The optional KMS provider validates
the file and exact display objects, retains its capture policy, and ties
revocation to the creating DRM file's final close. Providers remain responsible
for current authorization when later image operations are admitted.

The returned capture descriptor represents final-image authority, not raw
planes or a primary DRM file. The separate control descriptor owns revocation
and has no pixel operations. Both are close-on-exec. Duplicates refer to the
same file: final control-file release revokes, whereas final capture-client
release only drops that client's ownership. Closing the creating DRM file
revokes even if clients retain both new descriptors. Poll reports ``POLLHUP``
after authority cleanup, not GPU completion or presentation.

Publication is failure-atomic with respect to the descriptor table. The
request is input-only and contains an explicit pointer to output storage.
The kernel reserves both descriptors, creates the provider's checked pair,
and copies both numbers before installing either file. Failure releases
reservations and unpublished files. A partial user-memory copy may leave
numbers in output storage, but callers must ignore all output on failure.
Neither number identifies a published capture file until the ioctl succeeds.

Describing an image configuration
================================

``DRM_IOCTL_CAPTURE_DESCRIBE`` asks the capture file's provider what image
configuration it currently offers. The reply gives its visible dimensions,
DRM pixel format and modifier, maximum requests per stream, and a name for
that offer. It does not allocate images or start rendering. Destination
allocation details, including strides and exporter compatibility, still need
separate validation before delivery.

The name belongs to that client, not to a kernel address or a global authority
registry. Repeated queries keep the same name while the configuration is
unchanged. An ordinary content update does not change it, but a new mode or
route interval does, even when the dimensions stay the same. Stream creation
must subsequently recheck both the named configuration and current permission;
remembering the name does not preserve either. Existing streams retain their
own lifetime rules when another offer is queried.

All fields are output, including reserved fields returned as zero. A bad
output pointer may cause a partial copy, so callers must ignore the reply on
failure. Retrying describes the current configuration without consuming the
previous offer or any image-storage credit. A revoked grant returns
``EKEYREVOKED``. Providers may reject a description when the output is inactive
or its current content is outside the recipient's permission.

Creating and destroying streams
===============================

``DRM_IOCTL_CAPTURE_CREATE_STREAM`` supplies the offer name, a request
capacity and a new caller-chosen stream name. Names are nonzero and increase
within the client file, including across its duplicated descriptors. A
successful name is never reused, even after its stream is destroyed. The
provider checks the exact offered configuration and current permission while
reserving bounded resources. A failed creation does not consume the name.

Creation is input-only: no user-memory write or descriptor installation
follows admission. The stream belongs to the client file independently of
later description queries. Creating it does not queue capture demand or
establish continuing authority over future pixels.

``DRM_IOCTL_CAPTURE_DESTROY_STREAM`` removes one stream without revoking
siblings. Destruction remains available after a modeset or revocation, and
final client-file release destroys its remaining streams. Successful explicit
destruction ends that stream's destination writes; ``EBUSY`` leaves cleanup
retryable while access ends. Final file release instead abandons observation
without waiting for detached exporter access. Neither operation establishes
completion of shared rendering or other users of the same allocation.

Submitting and receiving output
==============================

``DRM_IOCTL_CAPTURE_QUEUE_OUTPUT`` supplies a stream-local increasing request
name, a registered destination name and an optional sync-file reuse descriptor.
The transport resolves and retains the fence before provider admission. Success
has no later copyout or descriptor installation; rejection consumes neither the
name nor a request slot. Accepted work retains the exact allocation even if its
registered name is removed. The provider rechecks its layout and permission.

The recipient must exclude competing access until terminal completion or
successful explicit stream destruction. A reuse fence describes prior work,
not exclusive future access. CastKMS rejects known current-source aliases by
comparing backing reservations, but different reservations do not prove physical
independence. The check does not prevent later KMS selection of that storage.
Destination reuse and exporter waits retain no compositor source.

``DRM_IOCTL_CAPTURE_DEQUEUE`` copies one terminal result to an explicit output
address before acknowledging it. ``EFAULT`` retains the record and request slot;
ignore partially copied bytes. A duplicate descriptor may consume the same queue.
``EAGAIN`` means no result is available. A terminal capture error, including
``EAGAIN``, is instead returned in the result's status with a successful ioctl.
Use an ioctl wrapper that preserves ``EAGAIN``: libdrm's ``drmIoctl()`` retries it
internally and is unsuitable for these nonblocking observations. Poll indicates
availability without reserving a result. ``POLLIN`` and ``POLLHUP`` may coexist.

Successful output carries the original image-production timestamp in monotonic
nanoseconds, not dequeue time or a presentation event. All destination writes
and cache maintenance for the attempt have ended before a terminal result is
published, including on failure. Failed output may contain partial pixels and
is not a valid image. No source descriptor or output fence is published.

``DRM_IOCTL_CAPTURE_CANCEL`` requests cancellation without acknowledging either
the result or storage reuse. It remains usable after revocation. A request whose
exporter access is already running stays pending until that access ends; no
completion is fabricated to acknowledge cancellation. Shared source rendering
and downstream users remain independent. Already cancelled or terminal requests
return ``EALREADY``; absent names return ``ENOENT``.

The wire adapters call ``drm_capture_client_queue_output()``,
``drm_capture_client_dequeue()`` and ``drm_capture_client_cancel()``. Kernel C
consumers use those entry points without userspace memory or descriptor lookup;
Rust ``ClientStream`` exposes the same operations with retained file references.
CastKMS delegates all three to its existing client queue. The transport owns no
provider pixel policy, request accounting or renderer selection.

Kernel providers
================

``drm_capture_create_file_grant()`` performs the same issuance without
reserving or installing descriptors. The dispatcher checks device association,
provider participation and the identity of the returned file pair. The provider
owns master, target and creator-lifetime policy; the transport does not grant
permission merely because mode objects were found.

Rust KMS providers implement the optional ``create_capture_grant`` callback,
which receives registered device access and an open file of the nominated
driver type. ``Device::create_capture_grant`` returns a checked ``FilePair``.
Ordinary kernel capture consumers continue using authority, stream and request
operations directly without manufacturing a DRM file or userspace descriptors.

Creator lifetime is available to C and Rust providers independently of files.
``drm_capture_creator_create()`` allocates a bounded collection of grants;
``drm_capture_creator_register()`` retains an already-authorized grant in that
collection. Closing the creator revokes the remaining grants outside the
tracking lock. Removing a registration only removes tracking; it does not
revoke the grant. Registrations may survive creator close without extending
the lifetime that allows new grants. Rust ``Creator`` and ``Registration``
provide the corresponding unique owners and perform cleanup on drop.
The provider chooses the limit and authorizes issuance before registration.

Client owners optionally implement ``describe``. The native file layer serializes
callbacks and checks the returned metadata; the provider checks current display
permission. ``drm_capture_client_describe()`` and Rust ``Description::query()``
perform the same query entirely in kernel memory. CastKMS's file callback uses
its transport-independent negotiation object above the permission provider,
so exposing the ioctl does not introduce another policy implementation.

Optional ``open_stream`` and ``close_stream`` callbacks use that same serialized
file layer. Opening requires both callbacks. Kernel callers use
``drm_capture_client_open_stream()`` and ``drm_capture_client_close_stream()``;
Rust ``ClientStream`` retains the file and attempts closure on drop. Explicit
close errors remain retryable. CastKMS forwards these callbacks to its
transport-independent client registry, also used by direct kernel consumers.

Both C and Rust providers can use ``drm_capture_resources`` for stream and
destination names. The table reserves a fixed number of slots and rejects
zero, repeated or decreasing names. Failed construction does not consume a
name; removal makes a slot available without making its old name reusable.
The provider serializes access and owns the values in those slots, including
their cleanup. Rust ``Resources<T>`` pairs that native table with typed values
and exclusive borrows. Neither layer infers capture permission from a name.

Drivers without capture leave the provider absent. This is not a requirement
for native GPU drivers used by a userspace renderer, and it does not enable
preparation or delegated rendering on another modesetting driver.

Destination registration uses the same kernel and file boundary. The generic
description borrows up to four DMA-BUF image planes; shared validation checks
metadata shape and each export's write access. Providers validate the complete
format/modifier layout and resource limits and acquire their own references.
Rust ``Destination`` borrows the buffers, and ``ClientDestination`` retains a
file-backed cleanup obligation without acquiring revocation ownership.

The input-only destination ioctls resolve all descriptors before admission.
Repeated numbers in one request use the first resolved allocation, and every
temporary reference is released after the provider returns. CastKMS accepts
single-plane HOST_V1 images through its existing checked image and client
registry. It currently bounds registrations to 16 per client and allocations
to 512 MiB each, independently of stream request depth and private result
storage. The complete described image span has the same bound, including row
padding. A larger bounded allocation may back a smaller image view.
These operations do not queue a capture or deliver pixels. Unregistering a
name is not storage revocation or GPU completion, and cleanup remains available
after capture revocation. Registration does not guarantee that a later exporter
mapping or GPU import will succeed.

.. kernel-doc:: include/uapi/drm/drm_capture.h
