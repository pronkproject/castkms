// SPDX-License-Identifier: GPL-2.0-only

/* Discover accepted constraints without enabling video or obtaining pixels. */
#include "fixture.h"
#include <fcntl.h>
#include <sys/ioctl.h>
#include <unistd.h>
#include "../../../../include/uapi/drm/drm_constraints.h"

int main(int argc, char **argv)
{
	struct drm_mode_list_constraints query = { 0 };
	struct drm_mode_constraints_list *list;
	struct drm_mode_constraints *entry;
	drmModeRes *resources;
	int fd, reader;

	CHECK(argc == 2);
	fd = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0);
	acquire_master(fd);
	CHECK(drmIsMaster(fd) == 1);
	resources = drmModeGetResources(fd);
	CHECK(resources && resources->count_crtcs > 0);
	query.crtc_id = resources->crtcs[0];
	CHECK(ioctl(fd, DRM_IOCTL_MODE_LIST_CONSTRAINTS, &query) == 0);
	CHECK(query.size >= sizeof(*list) && query.generation);
	list = calloc(1, query.size);
	CHECK(list);
	query.data = (uintptr_t)list;
	CHECK(ioctl(fd, DRM_IOCTL_MODE_LIST_CONSTRAINTS, &query) == 0);
	CHECK(list->version == DRM_MODE_CONSTRAINTS_VERSION);
	CHECK(list->length == query.size && list->generation == query.generation);
	CHECK(list->count_entries == 1 && list->entry_size == sizeof(*entry));
	CHECK(list->entries_offset <= list->length - sizeof(*entry));
	entry = (void *)((char *)list + list->entries_offset);
	CHECK(entry->id && entry->id == list->selected_id);
	CHECK(entry->flags == DRM_MODE_CONSTRAINTS_SELECTABLE);
	reader = open(argv[1], O_RDONLY | O_CLOEXEC);
	CHECK(reader >= 0 && !drmIsMaster(reader));
	errno = 0;
	CHECK(ioctl(reader, DRM_IOCTL_MODE_LIST_CONSTRAINTS, &query) < 0);
	CHECK(errno == EACCES);
	CHECK(close(reader) == 0);
	free(list);
	drmModeFreeResources(resources);
	CHECK(close(fd) == 0);
	puts("PASS: fixed default constraints discovery without active video");
	return 0;
}
