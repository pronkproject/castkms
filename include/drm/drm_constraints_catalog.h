/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_CATALOG_H__
#define __DRM_CONSTRAINTS_CATALOG_H__

#include <linux/types.h>

struct drm_constraints_catalog;
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
 * A bounded output catalog in one device identity domain. The initial entry
 * establishes a nonzero selected ID; the caller validates default readiness
 * and output scope before construction. The catalog owns references, not pixel
 * permissions. Every operation requires a live catalog reference and may sleep.
 * Entries must belong to the same domain and CRTC as the initial entry.
 */
struct drm_constraints_catalog *
drm_constraints_catalog_create(struct drm_constraints_domain *domain,
			       struct drm_constraints_entry *initial, unsigned int limit);
struct drm_constraints_catalog *
drm_constraints_catalog_get(struct drm_constraints_catalog *catalog);
void drm_constraints_catalog_put(struct drm_constraints_catalog *catalog);

/*
 * Provider operations. Add requires complete, ready resources; publication is
 * not itself selection. Withdraw marks an entry unavailable for selection.
 * Forget removes a withdrawn, unselected entry without invalidating
 * outstanding snapshots or accepted references. Suggestions are advisory;
 * zero clears the suggestion. No-op operations preserve the list generation.
 */
int drm_constraints_catalog_add(struct drm_constraints_catalog *catalog,
				struct drm_constraints_entry *entry);
int drm_constraints_catalog_withdraw(struct drm_constraints_catalog *catalog, u64 id);
int drm_constraints_catalog_forget(struct drm_constraints_catalog *catalog, u64 id);
int drm_constraints_catalog_suggest(struct drm_constraints_catalog *catalog, u64 id);

/*
 * Validate or accept an exact retained entry under the catalog lock. The caller
 * first stabilizes modesetting authority and all affected object state. Callback
 * lock order is caller locks -> catalog lock -> provider locks; callbacks must
 * not reenter catalog operations or acquire the caller locks again.
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
int drm_constraints_catalog_check(struct drm_constraints_catalog *catalog,
				  struct drm_constraints_entry *entry,
				  int (*check)(struct drm_constraints_entry *, void *), void *data);
int drm_constraints_catalog_accept(struct drm_constraints_catalog *catalog,
				   struct drm_constraints_entry *entry,
				   int (*install)(struct drm_constraints_entry *, void *), void *data);

/*
 * Returns one immutable snapshot, or ESTALE for a nonzero mismatching expected
 * generation. A snapshot owns all its entry references and reserves no future
 * selection. Views remain valid until snapshot_put(). No wire layout is implied.
 */
struct drm_constraints_snapshot *
drm_constraints_catalog_snapshot(struct drm_constraints_catalog *catalog, u64 generation);
void drm_constraints_snapshot_put(struct drm_constraints_snapshot *snapshot);
const struct drm_constraints_snapshot_info *
drm_constraints_snapshot_info(const struct drm_constraints_snapshot *snapshot);
const struct drm_constraints_listing *
drm_constraints_snapshot_entries(const struct drm_constraints_snapshot *snapshot);

#endif
