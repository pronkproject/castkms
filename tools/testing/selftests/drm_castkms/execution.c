// SPDX-License-Identifier: GPL-2.0-only

/* Read the renderer description without a capture grant or active display. */
#include "fixture.h"

#include <fcntl.h>
#include <string.h>
#include <unistd.h>

#include "../../../../include/uapi/drm/castkms_drm.h"

int main(int argc, char **argv)
{
	struct drm_castkms_execution description;
	drmModeObjectProperties *properties;
	drmModePropertyBlobRes *blob = NULL;
	drmModeRes *resources;
	uint32_t property_id = 0;
	int fd;

	CHECK(argc == 2);
	fd = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0);
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
	CHECK(drmModeObjectSetProperty(fd, resources->connectors[0],
				       DRM_MODE_OBJECT_CONNECTOR, property_id, 0) < 0);
	drmModeFreePropertyBlob(blob);
	drmModeFreeObjectProperties(properties);
	drmModeFreeResources(resources);
	close(fd);
	puts("PASS: fixed host execution description");
	return 0;
}
