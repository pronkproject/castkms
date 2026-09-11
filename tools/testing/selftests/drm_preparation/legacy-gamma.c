// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <errno.h>
#include <fcntl.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <xf86drm.h>
#include <xf86drmMode.h>
#include "../../../../include/uapi/drm/drm_prepare.h"
#include "../kselftest.h"

struct fixture {
	int fd;
	uint32_t crtc, count;
	uint16_t *values, *readback;
};

static void setup(struct fixture *f, const char *path)
{
	drmVersionPtr version;
	drmModeRes *resources;
	drmModeCrtc *crtc;
	uint64_t capability = 0;
	uint32_t i;

	f->fd = open(path, O_RDWR | O_CLOEXEC);
	if (f->fd < 0)
		ksft_exit_skip("Cannot open %s: %m\n", path);
	version = drmGetVersion(f->fd);
	if (!version || strcmp(version->name, "vkms"))
		ksft_exit_skip("Only run on an isolated VKMS device\n");
	drmFreeVersion(version);
	if (drmGetCap(f->fd, DRM_CAP_ATOMIC_PREPARATION, &capability) || !capability)
		ksft_exit_skip("VKMS preparation must be enabled\n");
	if (drmSetMaster(f->fd))
		ksft_exit_skip("Cannot acquire display ownership: %m\n");
	resources = drmModeGetResources(f->fd);
	if (!resources || resources->count_crtcs != 1)
		ksft_exit_skip("Requires a single-controller VKMS fixture\n");
	f->crtc = resources->crtcs[0];
	drmModeFreeResources(resources);
	crtc = drmModeGetCrtc(f->fd, f->crtc);
	if (!crtc || crtc->gamma_size <= 0 || crtc->gamma_size > 4096)
		ksft_exit_skip("Requires a bounded legacy gamma table\n");
	f->count = crtc->gamma_size;
	drmModeFreeCrtc(crtc);
	f->values = calloc(f->count, 3 * sizeof(*f->values));
	f->readback = calloc(f->count, 3 * sizeof(*f->readback));
	if (!f->values || !f->readback)
		ksft_exit_fail_msg("Cannot allocate gamma arrays\n");
	for (i = 0; i < f->count; i++) {
		f->values[i] = i * 7;
		f->values[i + f->count] = i * 11;
		f->values[i + 2 * f->count] = i * 13;
	}
}

static bool readback_matches(struct fixture *f)
{
	if (drmModeCrtcGetGamma(f->fd, f->crtc, f->count, f->readback,
			       f->readback + f->count, f->readback + 2 * f->count))
		ksft_exit_fail_msg("Cannot read gamma table: %m\n");
	return !memcmp(f->values, f->readback, 3 * f->count * sizeof(*f->values));
}

static void accepted_gamma(struct fixture *f)
{
	int ret = drmModeCrtcSetGamma(f->fd, f->crtc, f->count, f->values,
				     f->values + f->count, f->values + 2 * f->count);

	ksft_test_result(!ret && readback_matches(f),
			"Accepted legacy gamma returns all three requested component arrays\n");
}

int main(int argc, char **argv)
{
	struct fixture f = {};

	ksft_print_header();
	if (argc > 2)
		ksft_exit_fail_msg("Usage: %s [DEVICE]\n", argv[0]);
	setup(&f, argc > 1 ? argv[1] : "/dev/dri/card0");
	ksft_set_plan(1);
	accepted_gamma(&f);
	free(f.values);
	free(f.readback);
	close(f.fd);
	ksft_finished();
}
