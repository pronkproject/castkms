// SPDX-License-Identifier: GPL-2.0-only

/* Read the renderer description without a capture grant or active display. */
#include "fixture.h"

#include <fcntl.h>
#include <string.h>
#include <unistd.h>

#include "../../../../include/uapi/drm/castkms_drm.h"

static struct drm_castkms_execution check_description(int fd, int check_immutable)
{
	struct drm_castkms_execution description;
	drmModeObjectProperties *properties;
	drmModePropertyBlobRes *blob = NULL;
	drmModeRes *resources;
	uint32_t property_id = 0;
	resources = drmModeGetResources(fd);
	CHECK(resources && resources->count_connectors == 1);
	properties = drmModeObjectGetProperties(fd, resources->connectors[0],
					       DRM_MODE_OBJECT_CONNECTOR);
	CHECK(properties);
	for (uint32_t i = 0; i < properties->count_props; i++) {
		drmModePropertyRes *property = drmModeGetProperty(fd, properties->props[i]);

		CHECK(property);
		if (!strcmp(property->name, "CASTKMS_EXECUTION")) {
			CHECK(!property_id);
			CHECK(property->flags & DRM_MODE_PROP_BLOB);
			CHECK(property->flags & DRM_MODE_PROP_IMMUTABLE);
			property_id = property->prop_id;
			blob = drmModeGetPropertyBlob(fd, properties->prop_values[i]);
		}
		drmModeFreeProperty(property);
	}
	CHECK(property_id && blob && blob->length == sizeof(description));
	memcpy(&description, blob->data, sizeof(description));
	CHECK(description.version == DRM_CASTKMS_EXECUTION_VERSION);
	CHECK(description.profile == DRM_CASTKMS_EXECUTION_HOST_V1);
	CHECK(description.generation == 1);
	if (check_immutable) {
		CHECK(drmModeObjectSetProperty(fd, resources->connectors[0],
					       DRM_MODE_OBJECT_CONNECTOR, property_id, 0) < 0);
		CHECK(errno == EINVAL || errno == EOPNOTSUPP);
	}
	drmModeFreePropertyBlob(blob);
	drmModeFreeObjectProperties(properties);
	drmModeFreeResources(resources);
	return description;
}

int main(int argc, char **argv)
{
	struct drm_castkms_execution master_description, reader_description;
	int master, reader;

	CHECK(argc == 2);
	master = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(master >= 0 && drmIsMaster(master) == 1);
	reader = open(argv[1], O_RDONLY | O_CLOEXEC);
	CHECK(reader >= 0 && drmIsMaster(reader) == 0);
	master_description = check_description(master, 1);
	reader_description = check_description(reader, 0);
	CHECK(!memcmp(&master_description, &reader_description, sizeof(master_description)));
	CHECK(drmIsMaster(master) == 1 && drmIsMaster(reader) == 0);
	CHECK(close(reader) == 0);
	CHECK(close(master) == 0);
	puts("PASS: fixed host execution description without master authority");
	return 0;
}
