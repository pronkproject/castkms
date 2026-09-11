// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>
#include <xf86drm.h>
#include <xf86drmMode.h>
#include "../kselftest.h"

static uint32_t find_property(int fd, uint32_t crtc, const char *name)
{
	drmModeObjectProperties *props = drmModeObjectGetProperties(fd, crtc, DRM_MODE_OBJECT_CRTC);
	uint32_t id = 0;
	unsigned int i;

	if (!props)
		ksft_exit_fail_msg("Cannot query controller properties: %m\n");
	for (i = 0; i < props->count_props; i++) {
		drmModePropertyRes *prop = drmModeGetProperty(fd, props->props[i]);

		if (!prop)
			ksft_exit_fail_msg("Cannot query property: %m\n");
		if (!strcmp(prop->name, name))
			id = prop->prop_id;
		drmModeFreeProperty(prop);
	}
	drmModeFreeObjectProperties(props);
	return id;
}

static void expect_result(int fd, struct drm_mode_atomic *request, int error, const char *name)
{
	int ret;

	errno = 0;
	ret = drmIoctl(fd, DRM_IOCTL_MODE_ATOMIC, request);
	ksft_test_result(error ? ret == -1 && errno == error : ret == 0,
			"%s (return %d, errno %d)\n", name, ret, errno);
}

struct input_fixture {
	int fd;
	uint32_t crtc;
	uint32_t active;
};

struct input_request {
	struct drm_mode_atomic arg;
	uint32_t objects[2];
	uint32_t counts[2];
	uint32_t properties[2];
	uint64_t values[2];
};

static void init_request(struct input_request *request, const struct input_fixture *f)
{
	memset(request, 0, sizeof(*request));
	request->objects[0] = request->objects[1] = f->crtc;
	request->counts[0] = request->counts[1] = 1;
	request->properties[0] = request->properties[1] = f->active;
	request->arg = (struct drm_mode_atomic) {
		.flags = DRM_MODE_ATOMIC_TEST_ONLY | DRM_MODE_ATOMIC_ALLOW_MODESET,
		.count_objs = 1,
		.objs_ptr = (uintptr_t)request->objects,
		.count_props_ptr = (uintptr_t)request->counts,
		.props_ptr = (uintptr_t)request->properties,
		.prop_values_ptr = (uintptr_t)request->values,
	};
}

static void empty_arrays(const struct input_fixture *f)
{
	struct input_request request;

	init_request(&request, f);
	request.arg.count_objs = 0;
	request.arg.objs_ptr = request.arg.count_props_ptr = 1;
	request.arg.props_ptr = request.arg.prop_values_ptr = 1;
	expect_result(f->fd, &request.arg, 0, "Empty request does not read arrays");
	init_request(&request, f);
	request.counts[0] = 0;
	request.arg.props_ptr = request.arg.prop_values_ptr = 1;
	expect_result(f->fd, &request.arg, 0, "Zero properties do not read property arrays");
}

static void count_overflow(const struct input_fixture *f)
{
	struct input_request request;

	init_request(&request, f);
	request.counts[0] = UINT32_MAX;
	request.arg.count_objs = 2;
	expect_result(f->fd, &request.arg, EOVERFLOW, "Property count addition cannot wrap");
}

static void unreadable_arrays(const struct input_fixture *f)
{
	struct input_request request;

	init_request(&request, f);
	request.arg.objs_ptr = 1;
	expect_result(f->fd, &request.arg, EFAULT, "Unreadable object array is rejected");
	init_request(&request, f);
	request.arg.count_props_ptr = 1;
	expect_result(f->fd, &request.arg, EFAULT, "Unreadable count array is rejected");
	init_request(&request, f);
	request.arg.props_ptr = 1;
	expect_result(f->fd, &request.arg, EFAULT, "Unreadable property array is rejected");
	init_request(&request, f);
	request.arg.prop_values_ptr = 1;
	expect_result(f->fd, &request.arg, EFAULT, "Unreadable value array is rejected");
}

static void repeated_targets(const struct input_fixture *f)
{
	struct input_request request;

	init_request(&request, f);
	request.arg.count_objs = 2;
	expect_result(f->fd, &request.arg, 0, "Repeated targets keep both property segments");
}

int main(int argc, char **argv)
{
	const char *path = argc > 1 ? argv[1] : "/dev/dri/card0";
	struct input_fixture f;
	drmModeRes *resources;

	ksft_print_header();
	f.fd = open(path, O_RDWR | O_CLOEXEC);
	if (f.fd < 0)
		ksft_exit_skip("Cannot open %s: %m\n", path);
	if (drmSetClientCap(f.fd, DRM_CLIENT_CAP_ATOMIC, 1) || drmSetMaster(f.fd))
		ksft_exit_skip("Cannot become atomic KMS client: %m\n");
	resources = drmModeGetResources(f.fd);
	if (!resources || !resources->count_crtcs)
		ksft_exit_skip("No controller available\n");
	f.crtc = resources->crtcs[0];
	drmModeFreeResources(resources);
	f.active = find_property(f.fd, f.crtc, "ACTIVE");
	if (!f.active)
		ksft_exit_fail_msg("Atomic controller has no ACTIVE property\n");
	ksft_set_plan(8);
	empty_arrays(&f);
	count_overflow(&f);
	unreadable_arrays(&f);
	repeated_targets(&f);
	close(f.fd);
	ksft_finished();
}
