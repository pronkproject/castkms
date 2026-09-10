// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <xf86drm.h>
#include <xf86drmMode.h>
#include "../../../../include/uapi/drm/drm_prepare.h"
#include "../kselftest.h"

static uint32_t find_property(int fd, uint32_t crtc, const char *name)
{
	drmModeObjectProperties *props = drmModeObjectGetProperties(fd, crtc, DRM_MODE_OBJECT_CRTC);
	uint32_t id = 0;
	unsigned int i;

	if (!props)
		ksft_exit_fail_msg("Cannot query CRTC properties: %m\n");
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

static int issue(int fd, uint32_t crtc)
{
	struct drm_mode_prepare_replace arg = {
		.crtc_ids = (uintptr_t)&crtc,
		.count_crtcs = 1,
	};
	int ticket = drmIoctl(fd, DRM_IOCTL_MODE_PREPARE_REPLACE, &arg);

	if (ticket < 0)
		ksft_exit_fail_msg("Cannot issue preparation: %m\n");
	return ticket;
}

static int submit(int fd, uint32_t crtc, uint32_t property, int ticket, uint32_t flags)
{
	drmModeAtomicReq *req = drmModeAtomicAlloc();
	int ret;

	if (!req)
		ksft_exit_fail_msg("Cannot allocate atomic request\n");
	if (drmModeAtomicAddProperty(req, crtc, property, ticket) < 0)
		ksft_exit_fail_msg("Cannot add preparation property\n");
	ret = drmModeAtomicCommit(fd, req, flags, NULL);
	drmModeAtomicFree(req);
	return ret;
}

int main(int argc, char **argv)
{
	const char *path = argc > 1 ? argv[1] : "/dev/dri/card0";
	struct drm_mode_prepare_replace malformed = {};
	struct drm_prepare_query query = {};
	drmModeRes *resources;
	uint64_t capability = 0;
	uint32_t crtc, property, duplicates[2];
	int fd, ticket, stale, unrelated;

	ksft_print_header();
	fd = open(path, O_RDWR | O_CLOEXEC);
	if (fd < 0)
		ksft_exit_skip("Cannot open %s: %m\n", path);
	if (drmGetCap(fd, DRM_CAP_ATOMIC_PREPARATION, &capability) || !capability)
		ksft_exit_skip("Device does not support preparation\n");
	if (drmSetClientCap(fd, DRM_CLIENT_CAP_ATOMIC, 1) ||
	    drmSetClientCap(fd, DRM_CLIENT_CAP_ATOMIC_PREPARATION, 1) || drmSetMaster(fd))
		ksft_exit_skip("Cannot become preparation-aware KMS client: %m\n");
	resources = drmModeGetResources(fd);
	if (!resources || !resources->count_crtcs)
		ksft_exit_skip("No CRTC available\n");
	crtc = resources->crtcs[0];
	drmModeFreeResources(resources);
	property = find_property(fd, crtc, "PREPARE_FD");
	if (!property)
		ksft_exit_fail_msg("Preparation capability has no CRTC property\n");

	ksft_set_plan(9);
	errno = 0;
	ksft_test_result(drmIoctl(fd, DRM_IOCTL_MODE_PREPARE_REPLACE, &malformed) == -1 &&
			 errno == EINVAL, "Empty output set rejected\n");
	duplicates[0] = duplicates[1] = crtc;
	malformed.crtc_ids = (uintptr_t)duplicates;
	malformed.count_crtcs = 2;
	ksft_test_result(drmIoctl(fd, DRM_IOCTL_MODE_PREPARE_REPLACE, &malformed) == -1 &&
			 errno == EINVAL, "Duplicate output rejected\n");
	ticket = issue(fd, crtc);
	stale = issue(fd, crtc);
	ksft_test_result(fcntl(ticket, F_GETFD) & FD_CLOEXEC, "Ticket is close-on-exec\n");
	ksft_test_result(drmIoctl(ticket, DRM_IOCTL_PREPARE_QUERY, &query) == 0 &&
			 query.status == DRM_PREPARE_READY, "Ticket reports readiness\n");
	unrelated = open("/dev/null", O_RDONLY | O_CLOEXEC);
	if (unrelated < 0)
		ksft_exit_fail_msg("Cannot open unrelated descriptor: %m\n");
	ksft_test_result(submit(fd, crtc, property, unrelated, 0) == -EINVAL,
			 "Unrelated descriptor rejected\n");
	close(unrelated);
	ksft_test_result(submit(fd, crtc, property, ticket, DRM_MODE_ATOMIC_TEST_ONLY) == 0 &&
			 drmIoctl(ticket, DRM_IOCTL_PREPARE_QUERY, &query) == 0 &&
			 query.status == DRM_PREPARE_READY, "Test-only does not consume ticket\n");
	ksft_test_result(submit(fd, crtc, property, ticket, 0) == 0,
			 "Blocking unchanged-state commit accepts ticket\n");
	ksft_test_result(drmIoctl(ticket, DRM_IOCTL_PREPARE_QUERY, &query) == 0 &&
			 query.status == DRM_PREPARE_CONSUMED, "Accepted ticket reports consumption\n");
	ksft_test_result(submit(fd, crtc, property, stale, 0) == -ESTALE,
			 "Replacement invalidates earlier generation\n");
	close(stale);
	close(ticket);
	close(fd);
	ksft_finished();
}
