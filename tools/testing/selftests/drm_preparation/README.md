# Preparation protocol model

Run `./preparation-model.py` with Python 3, or use the kselftest Makefile. No
kernel build, DRM device, graphics library, or external Python package is needed.

This is an executable design experiment, not an implementation test or stable
interface. Each operation is a proposed serialized decision; Python execution
does not establish that DRM can implement that decision under its real locks.
The model uses a trusted executor: complete native coverage at release is an
asserted protocol obligation, not something a stock GPU driver enforces.

The initial cases cover:

- Claims unresolved until worker release, including submission racing cancel.
- READY with fixed but unsignaled native dependencies.
- Ticket seals retained across validation failure and TEST_ONLY.
- Atomic transfer to commit-owned seals independent of ticket close.
- Overlapping cohorts, competing acceptance, and last-close reopening.
- A-to-private-E source retirement independent of downstream E retention.
- Terminal worker loss without inventing native completion.
- Native error ending access without erasing the error status.
- One declared nominal schedule at independent staging depths of 1, 2, 4, 8.

The nominal schedule gives each admitted job a submission/release turn before
replacement. Its iteration count is not a measured frame rate or a fairness
proof. It demonstrates that preparation need not cancel every admitted job.

The model intentionally retains historical objects for assertions; their maps
are not proposed bounded production queues. It does not yet model authorization
domains and output claims, exported-allocation rollover, producer validity,
same-framebuffer content updates, result credits, asynchronous predecessor
resolution, request rebuilding, or exhaustive interleavings. It also does not
model GPU implicit dependencies or make crash-time ordering guarantees.

Passing these cases is an initial Phase 1A result, not completion of its gate.
Keep production UAPI numbers and native-driver assumptions out of this model.
