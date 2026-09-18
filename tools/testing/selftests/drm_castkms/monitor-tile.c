// SPDX-License-Identifier: GPL-2.0-only
/* Exercise transactional DisplayID tiled-monitor publication. */
#include <dirent.h>
#include <drm_fourcc.h>
#include <fcntl.h>
#include <linux/dma-buf.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

#include "fixture.h"

#include "../../../../include/uapi/drm/castkms_drm.h"
#include "../../../../include/uapi/drm/drm_capture.h"

_Static_assert(sizeof(struct drm_castkms_monitor_group_member) == 16,
	       "monitor group member ABI");
_Static_assert(sizeof(struct drm_castkms_monitor_group_mapping) == 24,
	       "monitor group mapping ABI");
_Static_assert(sizeof(struct drm_castkms_create_monitor_group) == 56,
	       "monitor group creation ABI");
_Static_assert(sizeof(struct drm_castkms_monitor_group_query) == 72,
	       "monitor group query ABI");
_Static_assert(sizeof(struct drm_castkms_monitor_group_capture_member) == 40,
	       "monitor group capture member ABI");
_Static_assert(sizeof(struct drm_castkms_create_monitor_group_capture) == 48,
	       "monitor group capture creation ABI");

struct tile_metadata {
	uint32_t group;
	uint32_t single_monitor;
	uint32_t horizontal_tiles;
	uint32_t vertical_tiles;
	uint32_t horizontal_location;
	uint32_t vertical_location;
	uint32_t width;
	uint32_t height;
};

static unsigned int open_files(void)
{
	DIR *directory = opendir("/proc/self/fd");
	struct dirent *entry;
	unsigned int count = 0;

	CHECK(directory);
	while ((entry = readdir(directory))) {
		char *end;
		long fd = strtol(entry->d_name, &end, 10);

		if (!*end && fd >= 0 && fd != dirfd(directory))
			count++;
	}
	CHECK(closedir(directory) == 0);
	return count;
}

static void checksum(unsigned char *block)
{
	unsigned char sum = 0;

	for (unsigned int i = 0; i < 127; i++)
		sum += block[i];
	block[127] = -sum;
}

static void fill_tile_edid(unsigned char edid[256], unsigned int horizontal_tiles,
			   unsigned int vertical_tiles,
			   unsigned int horizontal_location,
			   unsigned int vertical_location,
			   const char topology_id[9])
{
	static const unsigned char header[] = {
		0, 255, 255, 255, 255, 255, 255, 0
	};
	static const unsigned char display[] = {
		1, 3, 0x80, 160, 90, 120, 0x0a
	};
	static const unsigned char mode_1080p[] = {
		0x02, 0x3a, 0x80, 0x18, 0x71, 0x38, 0x2d, 0x40, 0x58,
		0x2c, 0x45, 0x00, 0x40, 0x84, 0x63, 0x00, 0x00, 0x1e
	};
	unsigned char *displayid;
	unsigned char sum = 0;

	CHECK(horizontal_tiles && horizontal_tiles <= 16);
	CHECK(vertical_tiles && vertical_tiles <= 16);
	CHECK(horizontal_location < horizontal_tiles);
	CHECK(vertical_location < vertical_tiles);
	memset(edid, 0, 256);
	memcpy(edid, header, sizeof(header));
	edid[8] = 0x31;
	edid[9] = 0xd8;
	edid[10] = 42;
	memcpy(edid + 18, display, sizeof(display));
	memset(edid + 38, 1, 16);
	memcpy(edid + 54, mode_1080p, sizeof(mode_1080p));
	edid[126] = 1;

	displayid = edid + 128;
	displayid[0] = 0x70;
	displayid[1] = 0x13;
	displayid[2] = 25;
	displayid[5] = 0x12;
	displayid[7] = 22;
	displayid[8] = 0x80;
	displayid[9] = ((horizontal_tiles - 1) << 4) | (vertical_tiles - 1);
	displayid[10] = (horizontal_location << 4) | vertical_location;
	displayid[12] = 0x7f;
	displayid[13] = 0x07;
	displayid[14] = 0x37;
	displayid[15] = 0x04;
	memcpy(displayid + 21, topology_id, 9);
	for (unsigned int i = 1; i < 30; i++)
		sum += displayid[i];
	displayid[30] = -sum;

	checksum(edid);
	checksum(displayid);
}

struct published_group {
	struct monitor_control monitor;
	struct drm_castkms_monitor_group_mapping
		mappings[DRM_CASTKMS_MONITOR_GROUP_MAX_MEMBERS];
	uint64_t group_id;
};

static struct tile_metadata tile_metadata(int fd, uint32_t connector_id);
static void probe_1080p(int fd, uint32_t connector_id);

static void check_disconnected(int fd, const drmModeRes *resources,
			       unsigned int first_connector,
			       unsigned int member_count)
{
	CHECK(first_connector + member_count <=
	      (unsigned int)resources->count_connectors);
	for (unsigned int i = 0; i < member_count; i++) {
		drmModeConnector *connector = drmModeGetConnector(fd,
			resources->connectors[first_connector + i]);

		CHECK(connector);
		CHECK(connector->connection == DRM_MODE_DISCONNECTED);
		drmModeFreeConnector(connector);
	}
}

static int publish_group(int fd, const drmModeRes *resources,
			 unsigned int first_connector,
			 unsigned int horizontal_tiles,
			 unsigned int vertical_tiles,
			 const char topology_id[9],
			 struct published_group *group)
{
	struct drm_castkms_monitor_group_member
		members[DRM_CASTKMS_MONITOR_GROUP_MAX_MEMBERS];
	struct drm_castkms_monitor_files files = { -1, -1 };
	unsigned char edids[DRM_CASTKMS_MONITOR_GROUP_MAX_MEMBERS][256];
	unsigned int member_count = horizontal_tiles * vertical_tiles;
	struct drm_castkms_create_monitor_group create = {
		.version = DRM_CASTKMS_MONITOR_GROUP_VERSION,
		.member_count = member_count,
		.members = (uintptr_t)members,
		.mappings = (uintptr_t)group->mappings,
		.files = (uintptr_t)&files,
	};

	CHECK(member_count <= DRM_CASTKMS_MONITOR_GROUP_MAX_MEMBERS);
	CHECK(first_connector + member_count <=
	      (unsigned int)resources->count_connectors);
	memset(group, 0, sizeof(*group));
	group->monitor.control_fd = -1;
	group->monitor.revoke_fd = -1;
	for (unsigned int i = 0; i < member_count; i++) {
		unsigned int x = i % horizontal_tiles;
		unsigned int y = i / horizontal_tiles;

		fill_tile_edid(edids[i], horizontal_tiles, vertical_tiles,
			       x, y, topology_id);
		members[i] = (struct drm_castkms_monitor_group_member) {
			.connector_id = resources->connectors[first_connector + i],
			.edid_size = sizeof(edids[i]),
			.edid_ptr = (uintptr_t)edids[i],
		};
	}
	if (ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_GROUP, &create) < 0)
		return -1;
	group->monitor = (struct monitor_control) {
		.control_fd = files.control_fd,
		.revoke_fd = files.revoke_fd,
	};
	group->group_id = group->mappings[0].group_id;
	return 0;
}

static void check_group(int fd, const drmModeRes *resources,
			unsigned int first_connector,
			unsigned int horizontal_tiles,
			unsigned int vertical_tiles,
			const char topology_id[9],
			const struct published_group *group)
{
	unsigned int member_count = horizontal_tiles * vertical_tiles;
	struct drm_castkms_monitor_group_mapping
		mappings[DRM_CASTKMS_MONITOR_GROUP_MAX_MEMBERS] = {0};
	struct drm_castkms_monitor_group_query query = {
		.version = DRM_CASTKMS_MONITOR_GROUP_VERSION,
		.mappings = (uintptr_t)mappings,
		.mapping_capacity = member_count,
	};

	CHECK(ioctl(group->monitor.control_fd,
		    DRM_IOCTL_CASTKMS_MONITOR_GROUP_QUERY, &query) == 0);
	CHECK(query.group_id == group->group_id);
	CHECK(query.member_count == member_count);
	CHECK(query.horizontal_tiles == horizontal_tiles);
	CHECK(query.vertical_tiles == vertical_tiles);
	CHECK(!memcmp(query.topology_id, topology_id, 9));
	for (unsigned int i = 0; i < member_count; i++) {
		struct tile_metadata tile;
		unsigned int x = i % horizontal_tiles;
		unsigned int y = i / horizontal_tiles;

		CHECK(mappings[i].group_id == group->group_id);
		CHECK(mappings[i].connector_id ==
		      resources->connectors[first_connector + i]);
		CHECK(mappings[i].horizontal_location == x);
		CHECK(mappings[i].vertical_location == y);
		probe_1080p(fd, mappings[i].connector_id);
		tile = tile_metadata(fd, mappings[i].connector_id);
		CHECK(tile.horizontal_tiles == horizontal_tiles);
		CHECK(tile.vertical_tiles == vertical_tiles);
		CHECK(tile.horizontal_location == x);
		CHECK(tile.vertical_location == y);
	}
}

static void exercise_repeated_group_capture(int fd, const drmModeRes *resources)
{
	static const char topology_id[] = "CASTWALL0";
	struct published_group wall;
	struct drm_castkms_monitor_group_capture_member
		first[DRM_CASTKMS_MONITOR_GROUP_MAX_MEMBERS] = {0};
	struct drm_castkms_monitor_group_capture_member
		second[DRM_CASTKMS_MONITOR_GROUP_MAX_MEMBERS] = {0};
	struct drm_castkms_create_monitor_group_capture capture = {
		.version = DRM_CASTKMS_MONITOR_GROUP_VERSION,
		.group_fd = -1,
		.member_capacity = DRM_CASTKMS_MONITOR_GROUP_MAX_MEMBERS,
		.members = (uintptr_t)first,
	};

	CHECK(publish_group(fd, resources, 0, 4, 2, topology_id, &wall) == 0);
	check_group(fd, resources, 0, 4, 2, topology_id, &wall);
	capture.group_fd = wall.monitor.control_fd;
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_GROUP_CAPTURE,
		    &capture) == 0);
	capture.members = (uintptr_t)second;
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_GROUP_CAPTURE,
		    &capture) == 0);
	for (unsigned int i = 0;
	     i < DRM_CASTKMS_MONITOR_GROUP_MAX_MEMBERS; i++) {
		CHECK(first[i].group_id == wall.group_id);
		CHECK(second[i].group_id == wall.group_id);
		CHECK(close(first[i].capture_fd) == 0);
		CHECK(close(first[i].control_fd) == 0);
		CHECK(close(second[i].capture_fd) == 0);
		CHECK(close(second[i].control_fd) == 0);
	}
	close_monitor(&wall.monitor);
	check_disconnected(fd, resources, 0, 8);
}

static void reject_incomplete_grid(int fd, const drmModeRes *resources)
{
	static const char topology_id[] = "CASTGRID0";
	struct drm_castkms_monitor_group_member members[4];
	struct drm_castkms_monitor_group_mapping mappings[4] = {0};
	struct drm_castkms_monitor_files files = { -1, -1 };
	unsigned char edids[4][256];
	struct drm_castkms_create_monitor_group create = {
		.version = DRM_CASTKMS_MONITOR_GROUP_VERSION,
		.member_count = 3,
		.members = (uintptr_t)members,
		.mappings = (uintptr_t)mappings,
		.files = (uintptr_t)&files,
	};

	for (unsigned int i = 0; i < 4; i++) {
		unsigned int location = i == 3 ? 2 : i;

		fill_tile_edid(edids[i], 2, 2, location % 2, location / 2,
			       topology_id);
		members[i] = (struct drm_castkms_monitor_group_member) {
			.connector_id = resources->connectors[2 + i],
			.edid_size = sizeof(edids[i]),
			.edid_ptr = (uintptr_t)edids[i],
		};
	}
	errno = 0;
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_GROUP, &create) == -1);
	CHECK(errno == EINVAL);
	CHECK(files.control_fd == -1 && files.revoke_fd == -1);
	check_disconnected(fd, resources, 2, 4);
	create.member_count = 4;
	errno = 0;
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_GROUP, &create) == -1);
	CHECK(errno == EINVAL);
	CHECK(files.control_fd == -1 && files.revoke_fd == -1);
	check_disconnected(fd, resources, 2, 4);
}

static void exercise_additional_layouts(int fd, const drmModeRes *resources)
{
	static const char vertical_id[] = "CASTVERT0";
	static const char grid_id[] = "CASTGRID0";
	static const char second_id[] = "CASTVERT1";
	static const char busy_id[] = "CASTBUSY0";
	struct published_group vertical, grid, second, replacement, rejected, reclaimed;
	struct drm_castkms_monitor_group_capture_member captures[2] = {0};
	struct drm_castkms_create_monitor_group_capture capture = {
		.version = DRM_CASTKMS_MONITOR_GROUP_VERSION,
		.member_capacity = 2,
		.members = (uintptr_t)captures,
	};
	uint64_t removed_group_id;

	CHECK(resources->count_connectors >= 8);
	reject_incomplete_grid(fd, resources);
	CHECK(publish_group(fd, resources, 0, 1, 2, vertical_id,
			    &vertical) == 0);
	CHECK(publish_group(fd, resources, 2, 2, 2, grid_id, &grid) == 0);
	errno = 0;
	CHECK(publish_group(fd, resources, 6, 1, 2, vertical_id,
			    &rejected) == -1);
	CHECK(errno == EEXIST);
	CHECK(rejected.monitor.control_fd == -1);
	CHECK(rejected.monitor.revoke_fd == -1);
	check_disconnected(fd, resources, 6, 2);
	CHECK(publish_group(fd, resources, 6, 1, 2, second_id, &second) == 0);
	CHECK(vertical.group_id != grid.group_id);
	CHECK(vertical.group_id != second.group_id);
	CHECK(grid.group_id != second.group_id);
	check_group(fd, resources, 0, 1, 2, vertical_id, &vertical);
	check_group(fd, resources, 2, 2, 2, grid_id, &grid);
	check_group(fd, resources, 6, 1, 2, second_id, &second);
	errno = 0;
	CHECK(publish_group(fd, resources, 0, 1, 2, busy_id,
			    &rejected) == -1);
	CHECK(errno == EBUSY);
	CHECK(rejected.monitor.control_fd == -1);
	CHECK(rejected.monitor.revoke_fd == -1);

	capture.group_fd = vertical.monitor.control_fd;
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_GROUP_CAPTURE,
		    &capture) == 0);
	removed_group_id = grid.group_id;
	close_monitor(&grid.monitor);
	for (unsigned int i = 0; i < 2; i++) {
		struct drm_capture_describe description = {0};

		CHECK(ioctl(captures[i].capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE,
			    &description) == -1);
		CHECK(errno == ENODEV);
	}
	check_group(fd, resources, 0, 1, 2, vertical_id, &vertical);
	check_group(fd, resources, 6, 1, 2, second_id, &second);
	CHECK(publish_group(fd, resources, 2, 2, 2, grid_id,
			    &replacement) == 0);
	CHECK(replacement.group_id > removed_group_id);
	check_group(fd, resources, 2, 2, 2, grid_id, &replacement);

	close_monitor(&vertical.monitor);
	for (unsigned int i = 0; i < 2; i++) {
		struct drm_capture_describe description = {0};

		CHECK(ioctl(captures[i].capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE,
			    &description) == -1);
		CHECK(errno == EKEYREVOKED);
		CHECK(close(captures[i].capture_fd) == 0);
		CHECK(close(captures[i].control_fd) == 0);
	}
	CHECK(publish_group(fd, resources, 0, 1, 2, busy_id,
			    &reclaimed) == 0);
	check_group(fd, resources, 0, 1, 2, busy_id, &reclaimed);
	check_group(fd, resources, 6, 1, 2, second_id, &second);
	close_monitor(&reclaimed.monitor);
	close_monitor(&replacement.monitor);
	close_monitor(&second.monitor);
	check_disconnected(fd, resources, 0, 8);
}

static struct tile_metadata tile_metadata(int fd, uint32_t connector_id)
{
	drmModeObjectProperties *properties;
	struct tile_metadata metadata = {0};
	uint64_t blob_id = 0;

	properties = drmModeObjectGetProperties(fd, connector_id,
						DRM_MODE_OBJECT_CONNECTOR);
	CHECK(properties);
	for (uint32_t i = 0; i < properties->count_props; i++) {
		drmModePropertyRes *property = drmModeGetProperty(fd,
							properties->props[i]);

		CHECK(property);
		if (!strcmp(property->name, "TILE"))
			blob_id = properties->prop_values[i];
		drmModeFreeProperty(property);
	}
	drmModeFreeObjectProperties(properties);
	CHECK(blob_id);

	drmModePropertyBlobRes *blob = drmModeGetPropertyBlob(fd, blob_id);

	CHECK(blob);
	CHECK(blob->data);
	CHECK(sscanf(blob->data, "%u:%u:%u:%u:%u:%u:%u:%u",
		     &metadata.group, &metadata.single_monitor,
		     &metadata.horizontal_tiles, &metadata.vertical_tiles,
		     &metadata.horizontal_location, &metadata.vertical_location,
		     &metadata.width, &metadata.height) == 8);
	drmModeFreePropertyBlob(blob);
	return metadata;
}

static void probe_1080p(int fd, uint32_t connector_id)
{
	drmModeConnector *connector = drmModeGetConnector(fd, connector_id);
	bool found = false;

	CHECK(connector);
	CHECK(connector->connection == DRM_MODE_CONNECTED);
	for (int i = 0; i < connector->count_modes; i++) {
		if (connector->modes[i].hdisplay == 1920 &&
		    connector->modes[i].vdisplay == 1080)
			found = true;
	}
	CHECK(found);
	drmModeFreeConnector(connector);
}

static drmModeModeInfo mode_1080p(int fd, uint32_t connector_id)
{
	drmModeConnector *connector = drmModeGetConnector(fd, connector_id);
	drmModeModeInfo mode = {0};

	CHECK(connector);
	for (int i = 0; i < connector->count_modes; i++) {
		if (connector->modes[i].hdisplay == 1920 &&
		    connector->modes[i].vdisplay == 1080) {
			mode = connector->modes[i];
			break;
		}
	}
	drmModeFreeConnector(connector);
	CHECK(mode.clock);
	return mode;
}

static void check_tile_pixels(int dma_fd, const struct buffer *buffer,
			      unsigned char value, bool cursor)
{
	struct dma_buf_sync sync = {
		.flags = DMA_BUF_SYNC_START | DMA_BUF_SYNC_READ,
	};
	unsigned char *pixels = mmap(NULL, buffer->dumb.size, PROT_READ,
				     MAP_SHARED, dma_fd, 0);

	CHECK(pixels != MAP_FAILED);
	CHECK(ioctl(dma_fd, DMA_BUF_IOCTL_SYNC, &sync) == 0);
	for (unsigned int y = 0; y < 1080; y++) {
		for (unsigned int x = 0; x < 1920 * 4; x++) {
			bool cursor_pixel = cursor && y >= 100 && y < 164 &&
					    x >= 100 * 4 && x < 164 * 4;
			unsigned char expected = cursor_pixel || x % 4 == 3 ?
					 0xff : value;

			CHECK(pixels[y * buffer->dumb.pitch + x] == expected);
		}
	}
	sync.flags = DMA_BUF_SYNC_END | DMA_BUF_SYNC_READ;
	CHECK(ioctl(dma_fd, DMA_BUF_IOCTL_SYNC, &sync) == 0);
	CHECK(munmap(pixels, buffer->dumb.size) == 0);
}

static void fill_shared_tiles(int fd, const struct buffer *buffer,
			      const unsigned char values[2])
{
	struct drm_mode_map_dumb map = { .handle = buffer->dumb.handle };
	unsigned char *pixels;

	CHECK(drmIoctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &map) == 0);
	pixels = mmap(NULL, buffer->dumb.size, PROT_READ | PROT_WRITE,
		      MAP_SHARED, fd, map.offset);
	CHECK(pixels != MAP_FAILED);
	for (unsigned int y = 0; y < 1080; y++) {
		memset(pixels + y * buffer->dumb.pitch, values[0], 1920 * 4);
		memset(pixels + y * buffer->dumb.pitch + 1920 * 4,
		       values[1], 1920 * 4);
	}
	CHECK(munmap(pixels, buffer->dumb.size) == 0);
}

static void
queue_tile_captures(const struct drm_castkms_monitor_group_capture_member captures[2],
		    uint64_t use_id)
{
	for (unsigned int i = 0; i < 2; i++) {
		struct drm_capture_queue_output queue = {
			.stream = 1,
			.use_id = use_id,
			.destination = 1,
			.reuse_fd = -1,
		};

		CHECK(ioctl(captures[i].capture_fd,
			    DRM_IOCTL_CAPTURE_QUEUE_OUTPUT, &queue) == 0);
	}
}

static void
dequeue_tile_captures(const struct drm_castkms_monitor_group_capture_member captures[2],
		      uint64_t use_id, struct drm_capture_result results[2])
{
	for (unsigned int i = 0; i < 2; i++) {
		struct pollfd event = {
			.fd = captures[i].capture_fd,
			.events = POLLIN,
		};
		struct drm_capture_dequeue dequeue = {
			.stream = 1,
			.result = (uintptr_t)&results[i],
		};

		memset(&results[i], 0, sizeof(results[i]));
		CHECK(poll(&event, 1, 5000) == 1);
		CHECK(event.revents & POLLIN);
		CHECK(!(event.revents & (POLLERR | POLLHUP | POLLNVAL)));
		CHECK(ioctl(captures[i].capture_fd, DRM_IOCTL_CAPTURE_DEQUEUE,
			    &dequeue) == 0);
		CHECK(results[i].use_id == use_id && results[i].status == 0);
		CHECK(results[i].completed_at_ns && !results[i].reserved);
	}
}

static void exercise_tile_pixels(int fd, const drmModeRes *resources,
				 const struct drm_castkms_monitor_group_capture_member captures[2])
{
	static const unsigned char values[2] = { 0x31, 0x72 };
	static const unsigned char shared_values[2] = { 0x19, 0xa4 };
	struct drm_capture_describe descriptions[2] = {0};
	struct drm_capture_result results[2] = {0};
	struct buffer sources[2], destinations[2], shared, cursor;
	drmModeModeInfo modes[2];
	int destination_fds[2];

	for (unsigned int i = 0; i < 2; i++) {
		struct drm_capture_create_stream stream = {
			.id = 1,
			.capacity = 1,
		};
		struct drm_capture_register_destination destination = {
			.id = 1,
			.width = 1920,
			.height = 1080,
			.format = DRM_FORMAT_XRGB8888,
			.num_planes = 1,
			.modifier = DRM_FORMAT_MOD_LINEAR,
		};

		modes[i] = mode_1080p(fd, resources->connectors[i]);
		sources[i] = create_buffer(fd, 1920, 1080, values[i]);
		destinations[i] = create_buffer(fd, 1920, 1080, 0);
		CHECK(drmPrimeHandleToFD(fd, destinations[i].dumb.handle,
					 DRM_CLOEXEC | DRM_RDWR,
					 &destination_fds[i]) == 0);
		CHECK(drmModeSetCrtc(fd, resources->crtcs[i], sources[i].fb, 0, 0,
				     &resources->connectors[i], 1, &modes[i]) == 0);
		CHECK(ioctl(captures[i].capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE,
			    &descriptions[i]) == 0);
		stream.offer = descriptions[i].id;
		CHECK(ioctl(captures[i].capture_fd,
			    DRM_IOCTL_CAPTURE_CREATE_STREAM, &stream) == 0);
		destination.fds[0] = destination_fds[i];
		destination.strides[0] = destinations[i].dumb.pitch;
		CHECK(ioctl(captures[i].capture_fd,
			    DRM_IOCTL_CAPTURE_REGISTER_DESTINATION,
			    &destination) == 0);
	}
	queue_tile_captures(captures, 1);
	dequeue_tile_captures(captures, 1, results);
	for (unsigned int i = 0; i < 2; i++) {
		check_tile_pixels(destination_fds[i], &destinations[i], values[i], false);
	}

	shared = create_buffer(fd, 3840, 1080, 0);
	fill_shared_tiles(fd, &shared, shared_values);
	for (unsigned int i = 0; i < 2; i++) {
		CHECK(drmModeSetCrtc(fd, resources->crtcs[i], shared.fb,
				     i * 1920, 0, &resources->connectors[i],
				     1, &modes[i]) == 0);
	}
	queue_tile_captures(captures, 2);
	dequeue_tile_captures(captures, 2, results);
	for (unsigned int i = 0; i < 2; i++) {
		check_tile_pixels(destination_fds[i], &destinations[i],
				  shared_values[i], false);
	}

	cursor = create_buffer(fd, 64, 64, 0xff);
	CHECK(drmModeSetCursor(fd, resources->crtcs[0], cursor.dumb.handle,
			       64, 64) == 0);
	CHECK(drmModeMoveCursor(fd, resources->crtcs[0], 100, 100) == 0);
	queue_tile_captures(captures, 3);
	dequeue_tile_captures(captures, 3, results);
	for (unsigned int i = 0; i < 2; i++) {
		check_tile_pixels(destination_fds[i], &destinations[i],
				  shared_values[i], i == 0);
	}
	CHECK(drmModeSetCursor(fd, resources->crtcs[0], 0, 0, 0) == 0);
	destroy_buffer(fd, &cursor);

	for (unsigned int i = 0; i < 2; i++) {
		struct drm_capture_unregister_destination destination = {
			.id = 1,
		};
		struct drm_capture_destroy_stream stream = { .id = 1 };

		CHECK(ioctl(captures[i].capture_fd,
			    DRM_IOCTL_CAPTURE_UNREGISTER_DESTINATION,
			    &destination) == 0);
		CHECK(ioctl(captures[i].capture_fd,
			    DRM_IOCTL_CAPTURE_DESTROY_STREAM, &stream) == 0);
		CHECK(close(destination_fds[i]) == 0);
		CHECK(drmModeSetCrtc(fd, resources->crtcs[i], 0, 0, 0,
				     NULL, 0, NULL) == 0);
		destroy_buffer(fd, &sources[i]);
		destroy_buffer(fd, &destinations[i]);
	}
	destroy_buffer(fd, &shared);
}

int main(int argc, char **argv)
{
	int fd;
	drmModeRes *resources;
	struct monitor_control monitor;
	struct drm_castkms_monitor_files files = { -1, -1 };
	struct drm_castkms_monitor_group_member members[2];
	struct drm_castkms_monitor_group_mapping mappings[2] = {0};
	struct drm_castkms_monitor_group_mapping queried_mappings[2] = {0};
	struct drm_castkms_monitor_group_mapping short_mapping = {0};
	struct drm_castkms_monitor_group_query query = {
		.version = DRM_CASTKMS_MONITOR_GROUP_VERSION,
		.mappings = (uintptr_t)queried_mappings,
		.mapping_capacity = 2,
	};
	struct drm_castkms_monitor_group_query metadata = {
		.version = DRM_CASTKMS_MONITOR_GROUP_VERSION,
	};
	struct drm_castkms_monitor_group_query undersized = {
		.version = DRM_CASTKMS_MONITOR_GROUP_VERSION,
		.mappings = (uintptr_t)&short_mapping,
		.mapping_capacity = 1,
	};
	struct drm_castkms_create_monitor_group create = {
		.version = DRM_CASTKMS_MONITOR_GROUP_VERSION,
		.member_count = 2,
		.members = (uintptr_t)members,
		.mappings = (uintptr_t)mappings,
		.files = (uintptr_t)&files,
	};
	struct drm_castkms_monitor_group_capture_member captures[2] = {
		{ .capture_fd = -1, .control_fd = -1 },
		{ .capture_fd = -1, .control_fd = -1 },
	};
	struct drm_castkms_create_monitor_group_capture capture = {
		.version = DRM_CASTKMS_MONITOR_GROUP_VERSION,
		.member_capacity = 2,
		.members = (uintptr_t)captures,
	};
	unsigned char edids[2][256];
	struct tile_metadata left, right;
	uint64_t first_group_id;
	unsigned int before;
	long page_size;
	void *partial;
	int helper, transferred;

	if (argc != 2) {
		fprintf(stderr, "SKIP: supply a disposable Rust CastKMS DRM node\n");
		return 4;
	}
	fd = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0);
	CHECK(drmSetClientCap(fd, DRM_CLIENT_CAP_UNIVERSAL_PLANES, 1) == 0);
	CHECK(drmSetClientCap(fd, DRM_CLIENT_CAP_ATOMIC, 1) == 0);
	CHECK(drmSetMaster(fd) == 0 || errno == EINVAL);
	resources = drmModeGetResources(fd);
	CHECK(resources);
	CHECK(resources->count_connectors >= 2);

	for (unsigned int i = 0; i < 2; i++) {
		fill_tile_edid(edids[i], 2, 1, i, 0, "CASTTILE0");
		members[i] = (struct drm_castkms_monitor_group_member) {
			.connector_id = resources->connectors[i],
			.edid_size = sizeof(edids[i]),
			.edid_ptr = (uintptr_t)edids[i],
		};
	}
	fill_tile_edid(edids[1], 2, 1, 0, 0, "CASTTILE0");
	errno = 0;
	CHECK(drmIoctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_GROUP, &create) == -1);
	CHECK(errno == EINVAL);
	CHECK(files.control_fd == -1 && files.revoke_fd == -1);
	fill_tile_edid(edids[1], 2, 1, 1, 0, "CASTTILE0");
	edids[1][54]++;
	checksum(edids[1]);
	errno = 0;
	CHECK(drmIoctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_GROUP, &create) == -1);
	CHECK(errno == EINVAL);
	CHECK(files.control_fd == -1 && files.revoke_fd == -1);
	fill_tile_edid(edids[1], 2, 1, 1, 0, "CASTTILE0");
	CHECK(drmIoctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_GROUP, &create) == 0);
	monitor = (struct monitor_control) {
		.control_fd = files.control_fd,
		.revoke_fd = files.revoke_fd,
	};
	transferred = fcntl(monitor.control_fd, F_DUPFD_CLOEXEC, 0);
	CHECK(transferred >= 0);
	CHECK(close(monitor.control_fd) == 0);
	monitor.control_fd = transferred;
	CHECK(mappings[0].connector_id == resources->connectors[0]);
	CHECK(mappings[0].group_id != 0);
	CHECK(mappings[0].horizontal_location == 0);
	CHECK(mappings[0].vertical_location == 0);
	CHECK(mappings[1].connector_id == resources->connectors[1]);
	CHECK(mappings[1].group_id == mappings[0].group_id);
	CHECK(mappings[1].horizontal_location == 1);
	CHECK(mappings[1].vertical_location == 0);
	CHECK(ioctl(monitor.control_fd, DRM_IOCTL_CASTKMS_MONITOR_GROUP_QUERY,
		    &metadata) == 0);
	CHECK(metadata.member_count == 2 && metadata.mappings == 0);
	errno = 0;
	CHECK(ioctl(monitor.control_fd, DRM_IOCTL_CASTKMS_MONITOR_GROUP_QUERY,
		    &undersized) == -1);
	CHECK(errno == ENOSPC);
	CHECK(short_mapping.group_id == 0);
	CHECK(ioctl(monitor.control_fd, DRM_IOCTL_CASTKMS_MONITOR_GROUP_QUERY,
		    &query) == 0);
	CHECK(query.version == DRM_CASTKMS_MONITOR_GROUP_VERSION);
	CHECK(query.flags == 0 && query.member_count == 2);
	CHECK(query.group_id == mappings[0].group_id);
	CHECK(query.horizontal_tiles == 2 && query.vertical_tiles == 1);
	CHECK(query.tile_width == 1920 && query.tile_height == 1080);
	CHECK(!memcmp(query.topology_id, "CASTTILE0", 9));
	CHECK(!memcmp(queried_mappings, mappings, sizeof(mappings)));
	CHECK(ioctl(monitor.control_fd, DRM_IOCTL_CASTKMS_MONITOR_GROUP_QUERY,
		    &query) == 0);
	first_group_id = query.group_id;
	capture.group_fd = monitor.revoke_fd;
	errno = 0;
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_GROUP_CAPTURE,
		    &capture) == -1);
	CHECK(errno == EBADF);
	capture.group_fd = monitor.control_fd;
	capture.member_capacity = 1;
	errno = 0;
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_GROUP_CAPTURE,
		    &capture) == -1);
	CHECK(errno == ENOSPC);
	CHECK(captures[0].capture_fd == -1 && captures[0].control_fd == -1);
	capture.member_capacity = 2;
	page_size = sysconf(_SC_PAGESIZE);
	CHECK(page_size > 0);
	partial = mmap(NULL, page_size * 2, PROT_READ | PROT_WRITE,
		       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	CHECK(partial != MAP_FAILED);
	CHECK(mprotect((char *)partial + page_size, page_size, PROT_NONE) == 0);
	before = open_files();
	capture.members = (uintptr_t)((char *)partial + page_size - sizeof(captures[0]));
	errno = 0;
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_GROUP_CAPTURE,
		    &capture) == -1);
	CHECK(errno == EFAULT);
	CHECK(open_files() == before);
	CHECK(munmap(partial, page_size * 2) == 0);
	capture.members = (uintptr_t)captures;
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_GROUP_CAPTURE,
		    &capture) == 0);
	for (unsigned int i = 0; i < 2; i++) {
		struct drm_capture_describe description = {0};

		CHECK(captures[i].group_id == first_group_id);
		CHECK(captures[i].crtc_id == resources->crtcs[i]);
		CHECK(captures[i].connector_id == resources->connectors[i]);
		CHECK(captures[i].horizontal_location == i);
		CHECK(captures[i].vertical_location == 0);
		CHECK(captures[i].reserved == 0 && captures[i].reserved2 == 0);
		CHECK(captures[i].capture_fd >= 0 && captures[i].control_fd >= 0);
		CHECK(fcntl(captures[i].capture_fd, F_GETFD) == FD_CLOEXEC);
		CHECK(fcntl(captures[i].control_fd, F_GETFD) == FD_CLOEXEC);
		CHECK(ioctl(captures[i].capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE,
			    &description) == -1);
		CHECK(errno == ENODEV);
	}
	exercise_tile_pixels(fd, resources, captures);
	probe_1080p(fd, resources->connectors[0]);
	probe_1080p(fd, resources->connectors[1]);
	left = tile_metadata(fd, resources->connectors[0]);
	right = tile_metadata(fd, resources->connectors[1]);
	CHECK(left.group != 0);
	CHECK(left.group == right.group);
	CHECK(left.single_monitor == 1 && right.single_monitor == 1);
	CHECK(left.horizontal_tiles == 2 && right.horizontal_tiles == 2);
	CHECK(left.vertical_tiles == 1 && right.vertical_tiles == 1);
	CHECK(left.horizontal_location == 0 && right.horizontal_location == 1);
	CHECK(left.vertical_location == 0 && right.vertical_location == 0);
	CHECK(left.width == 1920 && right.width == 1920);
	CHECK(left.height == 1080 && right.height == 1080);

	close_monitor(&monitor);
	for (unsigned int i = 0; i < 2; i++) {
		struct drm_capture_describe description = {0};

		CHECK(ioctl(captures[i].capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE,
			    &description) == -1);
		CHECK(errno == EKEYREVOKED);
		CHECK(close(captures[i].capture_fd) == 0);
		CHECK(close(captures[i].control_fd) == 0);
	}
	for (unsigned int i = 0; i < 2; i++) {
		drmModeConnector *connector = drmModeGetConnector(fd,
							 resources->connectors[i]);

		CHECK(connector);
		CHECK(connector->connection == DRM_MODE_DISCONNECTED);
		drmModeFreeConnector(connector);
	}

	/* Administrative publication neither requires nor displaces DRM master. */
	helper = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(helper >= 0 && !drmIsMaster(helper));
	files = (struct drm_castkms_monitor_files) { -1, -1 };
	memset(mappings, 0, sizeof(mappings));
	memset(queried_mappings, 0, sizeof(queried_mappings));
	query = (struct drm_castkms_monitor_group_query) {
		.version = DRM_CASTKMS_MONITOR_GROUP_VERSION,
		.mappings = (uintptr_t)queried_mappings,
		.mapping_capacity = 2,
	};
	create.flags = DRM_CASTKMS_MONITOR_CREATE_ADMIN;
	CHECK(ioctl(helper, DRM_IOCTL_CASTKMS_CREATE_MONITOR_GROUP, &create) == 0);
	CHECK(drmIsMaster(fd) && !drmIsMaster(helper));
	CHECK(mappings[0].group_id > first_group_id);
	CHECK(mappings[1].group_id == mappings[0].group_id);
	CHECK(ioctl(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_GROUP_QUERY,
		    &query) == 0);
	CHECK(query.group_id == mappings[0].group_id);
	CHECK(!memcmp(queried_mappings, mappings, sizeof(mappings)));
	monitor = (struct monitor_control) {
		.control_fd = files.control_fd,
		.revoke_fd = files.revoke_fd,
	};
	memset(captures, 0, sizeof(captures));
	capture = (struct drm_castkms_create_monitor_group_capture) {
		.version = DRM_CASTKMS_MONITOR_GROUP_VERSION,
		.flags = DRM_CASTKMS_MONITOR_GROUP_CAPTURE_ADMIN,
		.group_fd = monitor.control_fd,
		.member_capacity = 2,
		.members = (uintptr_t)captures,
	};
	CHECK(ioctl(helper, DRM_IOCTL_CASTKMS_CREATE_MONITOR_GROUP_CAPTURE,
		    &capture) == 0);
	for (unsigned int i = 0; i < 2; i++) {
		CHECK(captures[i].group_id == query.group_id);
		CHECK(close(captures[i].capture_fd) == 0);
		CHECK(close(captures[i].control_fd) == 0);
	}
	close_monitor(&monitor);
	CHECK(close(helper) == 0);
	exercise_additional_layouts(fd, resources);
	exercise_repeated_group_capture(fd, resources);
	drmModeFreeResources(resources);
	CHECK(close(fd) == 0);
	puts("PASS: DisplayID tiled monitor topology");
	return 0;
}
