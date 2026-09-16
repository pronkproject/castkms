/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_ENTRY_H__
#define __DRM_CONSTRAINTS_ENTRY_H__

#include <linux/types.h>

struct drm_constraints_description;
struct drm_constraints_domain;
struct drm_constraints_entry;
struct module;

/**
 * struct drm_constraints_entry_ops - retained provider resources
 * @owner: module containing callbacks, or NULL for built-in code
 * @release: release provider context after the final entry reference
 *
 * The context retains the backend implementing the immutable description.
 * It must remain valid independently of offer availability. Release may sleep.
 * The operations table must remain valid until release returns.
 */
struct drm_constraints_entry_ops {
	struct module *owner;
	void (*release)(void *data);
};

/*
 * One identity domain must span the device lifetime and all its outputs.
 * IDs are positive, never reused, and stop at U64_MAX. The limit bounds all
 * retained entries, including withdrawn entries referenced by accepted work.
 * Domains contain no DRM object references or authority; the device integration
 * owns scope validation. Calls may sleep and require an owned reference.
 */
struct drm_constraints_domain *drm_constraints_domain_create(unsigned int limit);
struct drm_constraints_domain *drm_constraints_domain_get(struct drm_constraints_domain *domain);
void drm_constraints_domain_put(struct drm_constraints_domain *domain);

/*
 * Creation retains the description and domain, and takes ownership of data
 * only on success. It does not publish an offer, validate readiness or grant
 * source access. The provider must supply an existing CRTC ID in the domain.
 */
struct drm_constraints_entry *
drm_constraints_entry_create(struct drm_constraints_domain *domain, u32 crtc_id,
			     struct drm_constraints_description *description,
			     const struct drm_constraints_entry_ops *ops, void *data);
struct drm_constraints_entry *drm_constraints_entry_get(struct drm_constraints_entry *entry);
void drm_constraints_entry_put(struct drm_constraints_entry *entry);
u64 drm_constraints_entry_id(const struct drm_constraints_entry *entry);
u32 drm_constraints_entry_crtc(const struct drm_constraints_entry *entry);
bool drm_constraints_entry_in_domain(const struct drm_constraints_entry *entry,
				     const struct drm_constraints_domain *domain);

/* Borrowed references; callers retain entry throughout use. */
struct drm_constraints_description *
drm_constraints_entry_description(const struct drm_constraints_entry *entry);
void *drm_constraints_entry_data(const struct drm_constraints_entry *entry);

#endif
