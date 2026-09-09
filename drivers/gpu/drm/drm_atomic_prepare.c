// SPDX-License-Identifier: GPL-2.0 OR MIT
/* Source-generation admission and submitted-reader handoff. */

#include <linux/dma-fence.h>
#include <linux/dma-fence-unwrap.h>
#include <linux/err.h>
#include <linux/export.h>
#include <linux/kref.h>
#include <linux/list.h>
#include <linux/limits.h>
#include <linux/mutex.h>
#include <linux/slab.h>

#include <drm/drm_atomic_prepare.h>

#include "drm_atomic_prepare_internal.h"

struct drm_prepare_domain {
	struct kref ref;
	struct mutex lock;
};

struct drm_prepare_read_claim {
	struct list_head link;
	struct drm_prepare_source *source;
	struct dma_fence *fence;
	bool released;
};

struct drm_prepare_source {
	struct kref ref;
	struct drm_prepare_domain *domain;
	struct list_head reads;
	unsigned int capacity;
	unsigned int count;
	unsigned int unresolved_claims;
	unsigned int admission_holds;
	bool sealed;
	bool claim_abandoned;
};

struct drm_prepare_admission_hold {
	struct kref ref;
	struct drm_prepare_source *source;
};

static void free_reads(struct list_head *reads)
{
	struct drm_prepare_read_claim *read, *next;

	list_for_each_entry_safe(read, next, reads, link) {
		list_del(&read->link);
		dma_fence_put(read->fence);
		kfree(read);
	}
}

static void domain_free(struct kref *ref)
{
	struct drm_prepare_domain *domain = container_of(ref, struct drm_prepare_domain, ref);

	mutex_destroy(&domain->lock);
	kfree(domain);
}

struct drm_prepare_domain *drm_prepare_domain_create(void)
{
	struct drm_prepare_domain *domain = kzalloc_obj(*domain);

	if (!domain)
		return ERR_PTR(-ENOMEM);
	kref_init(&domain->ref);
	mutex_init(&domain->lock);
	return domain;
}
EXPORT_SYMBOL_GPL(drm_prepare_domain_create);

struct drm_prepare_domain *drm_prepare_domain_get(struct drm_prepare_domain *domain)
{
	kref_get(&domain->ref);
	return domain;
}
EXPORT_SYMBOL_GPL(drm_prepare_domain_get);

void drm_prepare_domain_put(struct drm_prepare_domain *domain)
{
	kref_put(&domain->ref, domain_free);
}
EXPORT_SYMBOL_GPL(drm_prepare_domain_put);

static void source_free(struct kref *ref)
{
	struct drm_prepare_source *source = container_of(ref, struct drm_prepare_source, ref);

	/* Every unresolved claim owns a reference, so only released reads remain. */
	free_reads(&source->reads);
	drm_prepare_domain_put(source->domain);
	kfree(source);
}

struct drm_prepare_source *
drm_prepare_source_create_in(struct drm_prepare_domain *domain, unsigned int capacity)
{
	struct drm_prepare_source *source;

	if (!domain || !capacity)
		return ERR_PTR(-EINVAL);
	source = kzalloc_obj(*source);
	if (!source)
		return ERR_PTR(-ENOMEM);
	kref_init(&source->ref);
	source->domain = drm_prepare_domain_get(domain);
	INIT_LIST_HEAD(&source->reads);
	source->capacity = capacity;
	return source;
}
EXPORT_SYMBOL_GPL(drm_prepare_source_create_in);

struct drm_prepare_source *drm_prepare_source_create(unsigned int capacity)
{
	struct drm_prepare_domain *domain;
	struct drm_prepare_source *source;

	if (!capacity)
		return ERR_PTR(-EINVAL);
	domain = drm_prepare_domain_create();
	if (IS_ERR(domain))
		return ERR_CAST(domain);
	source = drm_prepare_source_create_in(domain, capacity);
	drm_prepare_domain_put(domain);
	return source;
}
EXPORT_SYMBOL_GPL(drm_prepare_source_create);

struct drm_prepare_source *drm_prepare_source_get(struct drm_prepare_source *source)
{
	kref_get(&source->ref);
	return source;
}
EXPORT_SYMBOL_GPL(drm_prepare_source_get);

void drm_prepare_source_put(struct drm_prepare_source *source)
{
	kref_put(&source->ref, source_free);
}
EXPORT_SYMBOL_GPL(drm_prepare_source_put);

struct drm_prepare_read_claim *drm_prepare_source_claim(struct drm_prepare_source *source)
{
	struct drm_prepare_read_claim *read, *next, *claim;
	LIST_HEAD(retired);
	int error = 0;

	claim = kzalloc_obj(*claim);
	if (!claim)
		return ERR_PTR(-ENOMEM);
	mutex_lock(&source->domain->lock);
	if (source->claim_abandoned) {
		error = -EIO;
		goto unlock;
	}
	if (source->sealed || source->admission_holds) {
		error = -EBUSY;
		goto unlock;
	}
	list_for_each_entry_safe(read, next, &source->reads, link) {
		if (read->released && dma_fence_is_signaled(read->fence)) {
			list_move_tail(&read->link, &retired);
			source->count--;
		}
	}
	if (source->count == source->capacity) {
		error = -EAGAIN;
		goto unlock;
	}
	claim->source = drm_prepare_source_get(source);
	list_add_tail(&claim->link, &source->reads);
	source->count++;
	source->unresolved_claims++;
unlock:
	mutex_unlock(&source->domain->lock);
	/* Native fence release callbacks never run under the accounting lock. */
	free_reads(&retired);
	if (error) {
		kfree(claim);
		return ERR_PTR(error);
	}
	return claim;
}
EXPORT_SYMBOL_GPL(drm_prepare_source_claim);

void drm_prepare_source_seal(struct drm_prepare_source *source)
{
	mutex_lock(&source->domain->lock);
	source->sealed = true;
	mutex_unlock(&source->domain->lock);
}
EXPORT_SYMBOL_GPL(drm_prepare_source_seal);

static int source_ready(struct drm_prepare_source *source, bool admission_held)
{
	if (source->claim_abandoned)
		return -EIO;
	return (source->sealed || admission_held) && !source->unresolved_claims ? 0 : -EAGAIN;
}

int drm_prepare_source_ready(struct drm_prepare_source *source)
{
	int error;

	mutex_lock(&source->domain->lock);
	error = source_ready(source, false);
	mutex_unlock(&source->domain->lock);
	return error;
}
EXPORT_SYMBOL_GPL(drm_prepare_source_ready);

struct drm_prepare_admission_hold *drm_prepare_source_hold_admission(struct drm_prepare_source *source)
{
	struct drm_prepare_admission_hold *hold;
	int error = 0;

	hold = kzalloc_obj(*hold);
	if (!hold)
		return ERR_PTR(-ENOMEM);
	mutex_lock(&source->domain->lock);
	if (source->claim_abandoned)
		error = -EIO;
	else if (source->admission_holds == UINT_MAX)
		error = -EOVERFLOW;
	else
		source->admission_holds++;
	mutex_unlock(&source->domain->lock);
	if (error) {
		kfree(hold);
		return ERR_PTR(error);
	}
	kref_init(&hold->ref);
	hold->source = drm_prepare_source_get(source);
	return hold;
}
EXPORT_SYMBOL_GPL(drm_prepare_source_hold_admission);

int drm_prepare_hold_sources(struct drm_prepare_source * const *sources,
			     struct drm_prepare_admission_hold **holds,
			     unsigned int count)
{
	struct drm_prepare_domain *domain;
	unsigned int i, allocated = 0;
	int error = 0;

	if (!count)
		return 0;
	domain = sources[0]->domain;
	for (i = 0; i < count; i++) {
		if (sources[i]->domain != domain)
			return -EXDEV;
	}
	for (i = 0; i < count; i++) {
		holds[i] = kzalloc_obj(*holds[i]);
		if (!holds[i]) {
			error = -ENOMEM;
			goto free_holds;
		}
		allocated++;
	}

	/* No source changes until every member passes under the shared domain lock. */
	mutex_lock(&domain->lock);
	for (i = 0; i < count; i++) {
		if (sources[i]->claim_abandoned) {
			error = -EIO;
			break;
		}
		if (sources[i]->admission_holds == UINT_MAX) {
			error = -EOVERFLOW;
			break;
		}
	}
	if (!error) {
		for (i = 0; i < count; i++) {
			sources[i]->admission_holds++;
			kref_init(&holds[i]->ref);
			holds[i]->source = drm_prepare_source_get(sources[i]);
		}
	}
	mutex_unlock(&domain->lock);
	if (!error)
		return 0;
free_holds:
	while (allocated)
		kfree(holds[--allocated]);
	return error;
}

struct drm_prepare_admission_hold *drm_prepare_admission_hold_get(struct drm_prepare_admission_hold *hold)
{
	kref_get(&hold->ref);
	return hold;
}
EXPORT_SYMBOL_GPL(drm_prepare_admission_hold_get);

static void admission_hold_free(struct kref *ref)
{
	struct drm_prepare_admission_hold *hold = container_of(ref, struct drm_prepare_admission_hold, ref);
	struct drm_prepare_source *source = hold->source;

	mutex_lock(&source->domain->lock);
	source->admission_holds--;
	mutex_unlock(&source->domain->lock);
	drm_prepare_source_put(source);
	kfree(hold);
}

void drm_prepare_admission_hold_put(struct drm_prepare_admission_hold *hold)
{
	kref_put(&hold->ref, admission_hold_free);
}
EXPORT_SYMBOL_GPL(drm_prepare_admission_hold_put);

int drm_prepare_admission_hold_ready(struct drm_prepare_admission_hold *hold)
{
	struct drm_prepare_source *source = hold->source;
	int error;

	mutex_lock(&source->domain->lock);
	error = source_ready(source, true);
	mutex_unlock(&source->domain->lock);
	return error;
}
EXPORT_SYMBOL_GPL(drm_prepare_admission_hold_ready);

static void finish_read(struct drm_prepare_read_claim *read, struct dma_fence *fence, bool abandoned)
{
	struct drm_prepare_source *source = read->source;
	LIST_HEAD(retired);

	mutex_lock(&source->domain->lock);
	source->unresolved_claims--;
	source->claim_abandoned |= abandoned;
	if (fence) {
		read->fence = dma_fence_get(fence);
		read->released = true;
	} else {
		list_move_tail(&read->link, &retired);
		source->count--;
	}
	mutex_unlock(&source->domain->lock);
	free_reads(&retired);
	/* Released entries belong to the source, not to an independent claim. */
	drm_prepare_source_put(source);
}

void drm_prepare_read_release(struct drm_prepare_read_claim *read, struct dma_fence *fence)
{
	finish_read(read, fence, false);
}
EXPORT_SYMBOL_GPL(drm_prepare_read_release);

void drm_prepare_read_abandon(struct drm_prepare_read_claim *read)
{
	finish_read(read, NULL, true);
}
EXPORT_SYMBOL_GPL(drm_prepare_read_abandon);

static int source_completion(struct drm_prepare_source *source, bool admission_held,
			     struct dma_fence **fence)
{
	struct dma_fence_unwrap *cursors = NULL;
	struct dma_fence **inputs = NULL, *merged = NULL;
	struct drm_prepare_read_claim *read;
	unsigned int count = 0;
	int error;

	mutex_lock(&source->domain->lock);
	error = source_ready(source, admission_held);
	if (error || !source->count)
		goto unlock;
	inputs = kmalloc_array(source->count, sizeof(*inputs), GFP_KERNEL);
	if (!inputs) {
		error = -ENOMEM;
		goto unlock;
	}
	cursors = kmalloc_array(source->count, sizeof(*cursors), GFP_KERNEL);
	if (!cursors) {
		error = -ENOMEM;
		goto unlock;
	}
	list_for_each_entry(read, &source->reads, link)
		inputs[count++] = read->fence;
unlock:
	mutex_unlock(&source->domain->lock);
	/* Permanent closure or the caller's hold excludes admission; no claim owner
	 * remains. The retained source keeps the immutable list and its inputs alive.
	 */
	if (!error && count) {
		merged = __dma_fence_unwrap_merge(count, inputs, cursors);
		if (!merged)
			error = -ENOMEM;
	}
	kfree(cursors);
	kfree(inputs);
	if (!error)
		*fence = merged;
	return error;
}

int drm_prepare_source_completion(struct drm_prepare_source *source, struct dma_fence **fence)
{
	return source_completion(source, false, fence);
}
EXPORT_SYMBOL_GPL(drm_prepare_source_completion);

int drm_prepare_admission_hold_completion(struct drm_prepare_admission_hold *hold, struct dma_fence **fence)
{
	return source_completion(hold->source, true, fence);
}
EXPORT_SYMBOL_GPL(drm_prepare_admission_hold_completion);
