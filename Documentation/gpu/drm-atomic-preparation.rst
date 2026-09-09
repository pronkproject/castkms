.. SPDX-License-Identifier: GPL-2.0 OR MIT

Source-read preparation groundwork
=================================

Userspace composition of a virtual display needs to distinguish a producer
finishing an image from a reader relinquishing access to that image. Rust's
framebuffer preparation callback collects producer dependencies before ordinary
atomic acceptance. The source accounting in ``drm_atomic_prepare.c`` addresses
the other side: claims already admitted to read one source generation.

The accounting is a kernel-only primitive. It neither changes atomic commit
semantics nor enables CastKMS capture. No preparation ioctl, multi-output
ticket, compositor negotiation or executor binding is provided yet.

Admission, release and completion
--------------------------------

A provider allocates one source generation with an explicit capacity. Before
claiming a read, it must establish pixel authority, retain the source storage
and have independent storage available for the result. Waiting for an encoder
or exported destination to become reusable must remain source-unbound.
The primitive cannot inspect or enforce a native GPU dependency graph.

Sealing permanently closes admission and cannot be undone. An admission hold
instead blocks new claims until its final reference is released, provided no
other hold or permanent seal remains. Releasing one of several overlapping
holds never reopens admission. Neither operation pauses GPU execution or
freezes pixel contents; both serialize with admission of new read claims.
A generation with admission closed becomes ready only after every admitted
claim has been released. Release consumes a claim and promises no more access
under it. It either reports ended synchronous access or supplies an
already-materialized native fence covering all submitted reads. Future
userspace submission is not a fence.

Readiness does not wait for those native fences to signal. It establishes a
fixed completion set with no unresolved userspace handoff. The provider can
obtain an owned native completion fence from that set. A failed native fence
still establishes ended access; it does not establish valid captured pixels.
The provider must keep its source storage and normal KMS retirement obligations
independently of the accounting allocation.

Capacity includes unresolved claims and released reads with pending native
completion. While admission remains open, completed readers are reclaimed when
another claim is attempted. The limit is chosen by the provider, not derived
from receiver frame rate or a universal queue depth.

Ownership and failure
---------------------

Each unresolved claim retains its source. Released native records belong to
the source itself, without retaining a reference back to it. Native fence
destruction and completion merging occur outside the accounting mutex.
Dropping the last external source reference does not fabricate claim release.

The Rust wrapper's ``ReadClaim`` is non-cloneable and release consumes it.
The name describes admission ownership, not proof that a read has occurred.
A ``PreparedSource`` retains either permanent closure or its own
``AdmissionHold`` reference; dropping the original hold cannot invalidate
that proof. Source-level readiness requires permanent closure, so an unrelated
hold owner cannot give another caller a preparation proof that disappears
when the hold is released.
Dropping an unreleased claim marks terminal service failure. Such a generation
never yields a prepared source, even after other claims are released. That
failure does not assert that unknown GPU work stopped; executor loss still
requires best-effort supervision and resource cleanup by the provider.

Reopening admission preserves unresolved claims and submitted native readers.
Releasing an admission hold does not cancel access already admitted. Later
preparation must still account for those readers before establishing readiness
and native completion.

The primitive must not be installed as a complete atomic preparation ticket:
multi-output cohorts, gap-free transfer to accepted commits, blocking internal
callers and teardown integration remain separate work. Kernel and Rust tests
exercise the primitive without publishing source buffers or touching a physical
display.
