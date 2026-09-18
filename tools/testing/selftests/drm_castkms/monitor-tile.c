// SPDX-License-Identifier: GPL-2.0-only
/* Exercise transactional DisplayID tiled-monitor publication. */
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

#include "fixture.h"

#include "../../../../include/uapi/drm/castkms_drm.h"

_Static_assert(sizeof(struct drm_castkms_monitor_group_member) == 16,
	       "monitor group member ABI");
_Static_assert(sizeof(struct drm_castkms_monitor_group_mapping) == 24,
	       "monitor group mapping ABI");
_Static_assert(sizeof(struct drm_castkms_create_monitor_group) == 56,
	       "monitor group creation ABI");
_Static_assert(sizeof(struct drm_castkms_monitor_group_query) == 64,
	       "monitor group query ABI");

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

static void checksum(unsigned char *block)
{
	unsigned char sum = 0;

	for (unsigned int i = 0; i < 127; i++)
		sum += block[i];
	block[127] = -sum;
}

static void fill_tile_edid(unsigned char edid[256], unsigned int location)
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

	CHECK(location < 2);
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
	displayid[9] = 0x10;
	displayid[10] = location << 4;
	displayid[12] = 0x7f;
	displayid[13] = 0x07;
	displayid[14] = 0x37;
	displayid[15] = 0x04;
	memcpy(displayid + 21, "CASTTILE0", 9);
	for (unsigned int i = 1; i < 30; i++)
		sum += displayid[i];
	displayid[30] = -sum;

	checksum(edid);
	checksum(displayid);
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

int main(int argc, char **argv)
{
	int fd;
	drmModeRes *resources;
	struct monitor_control monitor;
	struct drm_castkms_monitor_files files = { -1, -1 };
	struct drm_castkms_monitor_group_member members[2];
	struct drm_castkms_monitor_group_mapping mappings[2] = {0};
	struct drm_castkms_monitor_group_query query = {0};
	struct drm_castkms_create_monitor_group create = {
		.version = DRM_CASTKMS_MONITOR_GROUP_VERSION,
		.member_count = 2,
		.members = (uintptr_t)members,
		.mappings = (uintptr_t)mappings,
		.files = (uintptr_t)&files,
	};
	unsigned char edids[2][256];
	struct tile_metadata left, right;

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
		fill_tile_edid(edids[i], i);
		members[i] = (struct drm_castkms_monitor_group_member) {
			.connector_id = resources->connectors[i],
			.edid_size = sizeof(edids[i]),
			.edid_ptr = (uintptr_t)edids[i],
		};
	}
	fill_tile_edid(edids[1], 0);
	errno = 0;
	CHECK(drmIoctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_GROUP, &create) == -1);
	CHECK(errno == EINVAL);
	CHECK(files.control_fd == -1 && files.revoke_fd == -1);
	fill_tile_edid(edids[1], 1);
	CHECK(drmIoctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_GROUP, &create) == 0);
	monitor = (struct monitor_control) {
		.control_fd = files.control_fd,
		.revoke_fd = files.revoke_fd,
	};
	CHECK(mappings[0].connector_id == resources->connectors[0]);
	CHECK(mappings[0].group_id != 0);
	CHECK(mappings[0].horizontal_location == 0);
	CHECK(mappings[0].vertical_location == 0);
	CHECK(mappings[1].connector_id == resources->connectors[1]);
	CHECK(mappings[1].group_id == mappings[0].group_id);
	CHECK(mappings[1].horizontal_location == 1);
	CHECK(mappings[1].vertical_location == 0);
	CHECK(ioctl(monitor.control_fd, DRM_IOCTL_CASTKMS_MONITOR_GROUP_QUERY,
		    &query) == 0);
	CHECK(query.version == DRM_CASTKMS_MONITOR_GROUP_VERSION);
	CHECK(query.flags == 0 && query.member_count == 2);
	CHECK(query.group_id == mappings[0].group_id);
	CHECK(query.horizontal_tiles == 2 && query.vertical_tiles == 1);
	CHECK(query.tile_width == 1920 && query.tile_height == 1080);
	CHECK(!memcmp(query.topology_id, "CASTTILE0", 9));
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
		drmModeConnector *connector = drmModeGetConnector(fd,
							 resources->connectors[i]);

		CHECK(connector);
		CHECK(connector->connection == DRM_MODE_DISCONNECTED);
		drmModeFreeConnector(connector);
	}
	drmModeFreeResources(resources);
	CHECK(close(fd) == 0);
	puts("PASS: DisplayID tiled monitor topology");
	return 0;
}
