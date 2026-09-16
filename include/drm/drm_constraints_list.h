/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_LIST_H__
#define __DRM_CONSTRAINTS_LIST_H__

#include <linux/types.h>

struct drm_constraints_list;
struct drm_constraints_domain;
struct drm_constraints_entry;
struct drm_constraints_snapshot;

#define DRM_CONSTRAINTS_MAX_ENTRIES 64

struct drm_constraints_listing {
	struct drm_constraints_entry *entry;
	bool selectable;
};

struct drm_constraints_snapshot_info {
	u64 generation;
	u64 selected_id;
	u64 suggested_id;
	unsigned int count;
};

/*
 * A bounded output list in one device identity domain. The initial entry
 * establishes a nonzero selected ID; the caller validates default readiness
 * and output scope before construction. The list owns references, not pixel
 * permissions. Every operation requires a live list reference and may sleep.
 * Entries must belong to the same domain and CRTC as the initial entry.
 */
struct drm_constraints_list *
drm_constraints_list_create(struct drm_constraints_domain *domain,
			       struct drm_constraints_entry *initial, unsigned int limit);
struct drm_constraints_list *
drm_constraints_list_get(struct drm_constraints_list *list);
void drm_constraints_list_put(struct drm_constraints_list *list);

/*
 * Permanently exclude new selection and listing, synchronizing with acceptance
 * already in progress. Accepted bindings and retained snapshots remain valid.
 * This is not native completion or source revocation: the provider separately
 * closes source admission and quiesces output before restoring a default for
 * another owner. Do not hold locks needed by a list callback or call from
 * such a callback. Closing is idempotent and cannot fail on generation overflow.
 */
void drm_constraints_list_close(struct drm_constraints_list *list);

/*
 * Return an owned reference to accepted selection, including after closure.
 * Intended for state initialization/readback, not readiness or authorization.
 * Does not allocate and cannot fail for a live list.
 */
struct drm_constraints_entry *
drm_constraints_list_selected(struct drm_constraints_list *list);

/*
 * Resolve a positive ID to an owned candidate reference in this output list.
 * Zero returns EINVAL; unknown, withdrawn unselected or closed entries return
 * ESTALE. A withdrawn accepted entry remains resolvable for repeated selection.
 * Lookup reserves neither availability nor authority. Atomic acceptance must
 * recheck the retained entry, even if an earlier lookup succeeded.
 */
struct drm_constraints_entry *
drm_constraints_list_lookup(struct drm_constraints_list *list, u64 id);

/*
 * Provider operations. Add requires complete, ready resources; publication is
 * not itself selection. Withdraw marks an entry unavailable for selection.
 * Forget removes a withdrawn, unselected entry without invalidating
 * outstanding snapshots or accepted references. Suggestions are advisory;
 * zero clears the suggestion. No-op operations preserve the list generation.
 */
int drm_constraints_list_add(struct drm_constraints_list *list,
				struct drm_constraints_entry *entry);
int drm_constraints_list_withdraw(struct drm_constraints_list *list, u64 id);
int drm_constraints_list_forget(struct drm_constraints_list *list, u64 id);
int drm_constraints_list_suggest(struct drm_constraints_list *list, u64 id);

/*
 * Validate or accept an exact retained entry under the list lock. The caller
 * first stabilizes modesetting authority and all affected object state. Callback
 * lock order is caller locks -> list lock -> provider locks; callbacks must
 * not reenter list operations or acquire the caller locks again.
 *
 * Check has no reservation effect; its callback must have no external effects.
 * Accept repeats availability validation and calls install before changing the
 * selected ID. Install must perform all remaining fallible checks before any
 * state changes, return a negative errno without changing state on failure, or
 * return zero after irrevocable acceptance. No later activation acknowledgment
 * is required.
 * Accepted state must retain entry itself for delayed publication and reads.
 *
 * Repeating the selected entry preserves the generation, including when its
 * offer has been withdrawn. Provider readiness/authority checks still apply.
 * Neither operation grants source access or substitutes for full-scene checks.
 */
int drm_constraints_list_check(struct drm_constraints_list *list,
				  struct drm_constraints_entry *entry,
				  int (*check)(struct drm_constraints_entry *, void *), void *data);
int drm_constraints_list_accept(struct drm_constraints_list *list,
				   struct drm_constraints_entry *entry,
				   int (*install)(struct drm_constraints_entry *, void *), void *data);

/*
 * Returns one immutable snapshot, or ESTALE for a nonzero mismatching expected
 * generation. A snapshot owns all its entry references and reserves no future
 * selection. Views remain valid until snapshot_put(). No wire layout is implied.
 */
struct drm_constraints_snapshot *
drm_constraints_list_snapshot(struct drm_constraints_list *list, u64 generation);
void drm_constraints_snapshot_put(struct drm_constraints_snapshot *snapshot);
const struct drm_constraints_snapshot_info *
drm_constraints_snapshot_info(const struct drm_constraints_snapshot *snapshot);
const struct drm_constraints_listing *
drm_constraints_snapshot_entries(const struct drm_constraints_snapshot *snapshot);

#endif
