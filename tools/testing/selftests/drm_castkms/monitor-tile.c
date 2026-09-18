// SPDX-License-Identifier: GPL-2.0-only
/* Exercise DisplayID tiled-monitor publication through virtual monitor control. */
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

#include "fixture.h"

#include "../../../../include/uapi/drm/castkms_drm.h"

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

static struct monitor_control attach_tile(int fd,
					  uint32_t connector_id,
					  unsigned int location)
{
	struct drm_castkms_monitor_files files = { -1, -1 };
	struct drm_castkms_create_monitor_control create = {
		.connector_id = connector_id,
		.files = (uintptr_t)&files,
	};
	unsigned char edid[256];
	struct drm_castkms_monitor_attach attach = {
		.edid_ptr = (uintptr_t)edid,
		.edid_size = sizeof(edid),
	};

	fill_tile_edid(edid, location);
	CHECK(drmIoctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL, &create) == 0);
	CHECK(ioctl(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_ATTACH, &attach) == 0);
	return (struct monitor_control) {
		.control_fd = files.control_fd,
		.revoke_fd = files.revoke_fd,
	};
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
	struct monitor_control monitors[2];
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

	monitors[0] = attach_tile(fd, resources->connectors[0], 0);
	monitors[1] = attach_tile(fd, resources->connectors[1], 1);
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

	close_monitor(&monitors[1]);
	close_monitor(&monitors[0]);
	drmModeFreeResources(resources);
	CHECK(close(fd) == 0);
	puts("PASS: DisplayID tiled monitor topology");
	return 0;
}
