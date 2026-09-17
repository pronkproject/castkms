// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <kunit/visibility.h>
#include <linux/kref.h>
#include <linux/slab.h>
#include <linux/workqueue.h>
#include <drm/drm_auth.h>
#include <drm/drm_constraints_list.h>
#include <drm/drm_constraints_output.h>
#include <drm/drm_crtc.h>
#include <drm/drm_file.h>
#include <drm/drm_lease.h>
#include <uapi/drm/drm_constraints.h>

#include "drm_constraints_events.h"

struct constraints_output_events {
	struct drm_constraints_events *owner;
	struct drm_constraints_list *list;
	wait_queue_entry_t changes;
	struct drm_crtc *crtc;
	u64 sent_generation;
	bool sent_closed;
	atomic_t queued;
	struct drm_pending_event pending;
	struct drm_event_kms_constraints_list_changed payload;
};

struct drm_constraints_events {
	struct kref ref;
	struct drm_file *file;
	struct work_struct work;
	spinlock_t lock;
	bool ready;
	bool stopping;
	wait_queue_entry_t space;
	unsigned int count;
	unsigned int next_output;
	struct constraints_output_events outputs[];
};

static void events_free(struct kref *ref)
{
	struct drm_constraints_events *events =
		container_of(ref, struct drm_constraints_events, ref);

	/* List references and waiters have already been released by destroy. */
	kfree(events);
}

static void schedule_events(struct drm_constraints_events *events)
{
	unsigned long flags;

	spin_lock_irqsave(&events->lock, flags);
	if (events->ready && !events->stopping)
		queue_work(system_unbound_wq, &events->work);
	spin_unlock_irqrestore(&events->lock, flags);
}

static int list_changed(wait_queue_entry_t *wait, unsigned int mode, int flags, void *key)
{
	struct constraints_output_events *output =
		container_of(wait, struct constraints_output_events, changes);

	schedule_events(output->owner);
	return 0;
}

static int space_returned(wait_queue_entry_t *wait, unsigned int mode, int flags, void *key)
{
	struct drm_constraints_events *events =
		container_of(wait, struct drm_constraints_events, space);

	schedule_events(events);
	return 0;
}

static void event_released(struct drm_pending_event *pending)
{
	struct constraints_output_events *output =
		container_of(pending, struct constraints_output_events, pending);
	struct drm_constraints_events *events = output->owner;

	/* Return the embedded slot before requesting another generation. */
	atomic_set_release(&output->queued, 0);
	schedule_events(events);
	kref_put(&events->ref, events_free);
}

static bool queue_output(struct constraints_output_events *output, struct drm_master *master)
{
	struct drm_constraints_events *events = output->owner;
	struct drm_file *file = events->file;
	u64 generation = 0;
	bool closed;
	int ret;

	if (atomic_read_acquire(&output->queued))
		return false;
	ret = drm_constraints_list_observe(output->list, &generation);
	if (ret && ret != -ESTALE)
		return false;
	closed = ret == -ESTALE;
	if (closed ? output->sent_closed : generation == output->sent_generation)
		return false;
	/* Recheck visibility through publication, without nesting ID and list locks. */
	guard(mutex)(&master->dev->mode_config.idr_mutex);
	if (!drm_master_holds_object_locked(master, &output->crtc->base))
		return false;
	memset(&output->pending, 0, sizeof(output->pending));
	memset(&output->payload, 0, sizeof(output->payload));
	output->payload.base.type = DRM_EVENT_KMS_CONSTRAINTS_LIST_CHANGED;
	output->payload.base.length = sizeof(output->payload);
	output->payload.crtc_id = output->crtc->base.id;
	output->payload.generation = generation;
	output->payload.flags = closed ? DRM_KMS_CONSTRAINTS_LIST_CLOSED : 0;
	ret = drm_event_reserve_init_with_release(file->minor->dev, file, &output->pending,
						&output->payload.base, event_released);
	if (ret)
		return false;
	kref_get(&events->ref);
	atomic_set(&output->queued, 1);
	output->sent_generation = generation;
	output->sent_closed = closed;
	drm_send_event(file->minor->dev, &output->pending);
	return true;
}

static void send_changes(struct work_struct *work)
{
	struct drm_constraints_events *events =
		container_of(work, struct drm_constraints_events, work);
	struct drm_file *file = events->file;
	struct drm_master *master;
	bool was_current;
	unsigned int i, index, start = events->next_output;

	master = drm_file_get_master_snapshot(file, &was_current);
	if (!master)
		return;
	if (!was_current || !drm_master_lock_current_identity(master))
		goto put_master;
	if (file->master == master && file->is_master) {
		for (i = 0; i < events->count; i++) {
			index = (start + i) % events->count;
			if (queue_output(&events->outputs[index], master))
				events->next_output = (index + 1) % events->count;
		}
	}
	drm_master_unlock_current_identity(master);
put_master:
	drm_master_put(&master);
}

struct drm_constraints_events *drm_constraints_events_create(struct drm_file *file)
{
	struct drm_device *dev = file->minor->dev;
	struct drm_constraints_events *events;
	struct drm_constraints_list *list;
	struct constraints_output_events *output;
	struct drm_crtc *crtc;
	unsigned int count = 0;

	drm_for_each_crtc(crtc, dev)
		if (drm_constraints_crtc_list(crtc))
			count++;
	if (!count)
		return ERR_PTR(-EOPNOTSUPP);
	/* CRTC masks bound topology to 32 outputs; never allocate from user counts. */
	if (count > 32)
		return ERR_PTR(-E2BIG);
	events = kzalloc(struct_size(events, outputs, count), GFP_KERNEL_ACCOUNT);
	if (!events)
		return ERR_PTR(-ENOMEM);
	kref_init(&events->ref);
	spin_lock_init(&events->lock);
	INIT_WORK(&events->work, send_changes);
	events->file = file;
	init_waitqueue_func_entry(&events->space, space_returned);
	add_wait_queue(&file->event_space_wait, &events->space);
	drm_for_each_crtc(crtc, dev) {
		list = drm_constraints_crtc_list(crtc);
		if (!list)
			continue;
		output = &events->outputs[events->count++];
		output->owner = events;
		output->crtc = crtc;
		output->list = drm_constraints_list_get(list);
		init_waitqueue_func_entry(&output->changes, list_changed);
		add_wait_queue(drm_constraints_list_waitqueue(list), &output->changes);
		/* Register first. The final scan catches changes during initialization. */
		output->sent_closed =
			drm_constraints_list_observe(list, &output->sent_generation) != 0;
	}
	spin_lock_irq(&events->lock);
	events->ready = true;
	spin_unlock_irq(&events->lock);
	schedule_events(events);
	return events;
}
EXPORT_SYMBOL_IF_KUNIT(drm_constraints_events_create);

void drm_constraints_events_destroy(struct drm_constraints_events *events)
{
	unsigned long flags;
	unsigned int i;

	if (!events)
		return;
	spin_lock_irqsave(&events->lock, flags);
	events->stopping = true;
	spin_unlock_irqrestore(&events->lock, flags);
	for (i = 0; i < events->count; i++)
		remove_wait_queue(drm_constraints_list_waitqueue(events->outputs[i].list),
				  &events->outputs[i].changes);
	remove_wait_queue(&events->file->event_space_wait, &events->space);
	cancel_work_sync(&events->work);
	for (i = 0; i < events->count; i++)
		drm_constraints_list_put(events->outputs[i].list);
	kref_put(&events->ref, events_free);
}
EXPORT_SYMBOL_IF_KUNIT(drm_constraints_events_destroy);

#if IS_ENABLED(CONFIG_DRM_KUNIT_TEST)
void drm_constraints_events_flush(struct drm_constraints_events *events)
{
	flush_work(&events->work);
}
EXPORT_SYMBOL_IF_KUNIT(drm_constraints_events_flush);
#endif
