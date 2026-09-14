==================
DRM capture grants
==================

The experimental capture interface separates permission to receive a final
output image from modesetting, source-buffer access and rendering. Its current
public operation creates a grant; image negotiation and delivery operations
are not yet exposed on the returned capture file. The interface assignments
are development ABI, not upstream allocations.

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

Drivers without capture leave the provider absent. This is not a requirement
for native GPU drivers used by a userspace renderer, and it does not enable
preparation or delegated rendering on another modesetting driver.

.. kernel-doc:: include/uapi/drm/drm_capture.h
