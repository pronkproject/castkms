// SPDX-License-Identifier: GPL-2.0-only
/* Run only against an explicitly selected, disposable Rust CastKMS device. */
#include <fcntl.h>
#include <string.h>
#include <unistd.h>

#include "fixture.h"

static void check_vblank(int fd, const drmModeModeInfo *mode)
{
	drmVBlank first = { .request = { .type = DRM_VBLANK_RELATIVE, .sequence = 1 } };
	drmVBlank next = { .request = { .type = DRM_VBLANK_RELATIVE, .sequence = 3 } };
	uint64_t first_us, next_us, expected_us, elapsed_us;
	uint32_t frames;

	CHECK(drmWaitVBlank(fd, &first) == 0);
	CHECK(drmWaitVBlank(fd, &next) == 0);
	frames = next.reply.sequence - first.reply.sequence;
	CHECK(frames >= 3);
	first_us = (uint64_t)first.reply.tval_sec * 1000000 + first.reply.tval_usec;
	next_us = (uint64_t)next.reply.tval_sec * 1000000 + next.reply.tval_usec;
	CHECK(next_us > first_us);
	elapsed_us = next_us - first_us;
	expected_us = 1000ULL * mode->htotal * mode->vtotal * frames / mode->clock;
	/* Allow scheduling jitter, but distinguish a clock running at half the selected rate. */
	CHECK(elapsed_us >= expected_us * 3 / 4);
	CHECK(elapsed_us <= expected_us * 5 / 4);
}

int main(int argc, char **argv)
{
	drmModeRes *resources;
	drmModeConnector *connector;
	drmModeAtomicReq *req;
	drmModeCrtc *crtc;
	drmVersion *version;
	drmModeModeInfo *mode;
	drmModeModeInfo slow_mode;
	struct buffer a, b;
	uint32_t crtc_id, connector_id, plane_id, mode_id, slow_mode_id;
	int fd;

	if (argc != 2 && argc != 3) {
		fprintf(stderr, "SKIP: supply a disposable Rust CastKMS DRM node [DMA heap]\n");
		return 4;
	}
	fd = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0);
	version = drmGetVersion(fd);
	CHECK(version && !strcmp(version->name, "castkms"));
	CHECK(version->version_major == 0 && version->version_minor == 0);
	drmFreeVersion(version);
	CHECK(drmSetClientCap(fd, DRM_CLIENT_CAP_ATOMIC, 1) == 0);
	CHECK(drmSetMaster(fd) == 0);
	resources = drmModeGetResources(fd);
	CHECK(resources && resources->count_crtcs > 0 && resources->count_connectors > 0);
	crtc_id = resources->crtcs[0];
	connector_id = resources->connectors[0];
	connector = drmModeGetConnector(fd, connector_id);
	CHECK(connector && connector->connection == DRM_MODE_CONNECTED);
	CHECK(connector->count_modes > 0);
	mode = &connector->modes[0];
	plane_id = primary_plane(fd, 0);
	a = create_buffer(fd, mode->hdisplay, mode->vdisplay, 0x33);
	b = create_buffer(fd, mode->hdisplay, mode->vdisplay, 0x88);
	CHECK(drmModeCreatePropertyBlob(fd, mode, sizeof(*mode), &mode_id) == 0);
	req = drmModeAtomicAlloc();
	CHECK(req);
	property(fd, req, connector_id, DRM_MODE_OBJECT_CONNECTOR, "CRTC_ID", crtc_id);
	property(fd, req, crtc_id, DRM_MODE_OBJECT_CRTC, "ACTIVE", 1);
	property(fd, req, crtc_id, DRM_MODE_OBJECT_CRTC, "MODE_ID", mode_id);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "FB_ID", a.fb);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_ID", crtc_id);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "SRC_X", 0);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "SRC_Y", 0);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "SRC_W", mode->hdisplay << 16);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "SRC_H", mode->vdisplay << 16);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_X", 0);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_Y", 0);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_W", mode->hdisplay);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_H", mode->vdisplay);
	CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_ALLOW_MODESET |
				 DRM_MODE_ATOMIC_TEST_ONLY, NULL) == 0);
	crtc = drmModeGetCrtc(fd, crtc_id);
	CHECK(crtc && !crtc->mode_valid);
	drmModeFreeCrtc(crtc);
	CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_ALLOW_MODESET, NULL) == 0);
	drmModeAtomicFree(req);
	crtc = drmModeGetCrtc(fd, crtc_id);
	CHECK(crtc && crtc->mode_valid && crtc->buffer_id == a.fb);
	drmModeFreeCrtc(crtc);
	check_vblank(fd, mode);
	if (argc == 3) {
		struct buffer foreign = import_buffer(fd, argv[2], mode->hdisplay, mode->vdisplay);
		const uint32_t flags[] = { DRM_MODE_ATOMIC_TEST_ONLY, 0, DRM_MODE_ATOMIC_NONBLOCK };

		CHECK(drmModeSetCrtc(fd, crtc_id, foreign.fb, 0, 0, &connector_id, 1, mode) == 0);
		crtc = drmModeGetCrtc(fd, crtc_id);
		CHECK(crtc && crtc->mode_valid && crtc->buffer_id == foreign.fb);
		drmModeFreeCrtc(crtc);
		CHECK(drmModeSetCrtc(fd, crtc_id, a.fb, 0, 0, &connector_id, 1, mode) == 0);
		CHECK(drmModePageFlip(fd, crtc_id, foreign.fb, 0, NULL) == 0);
		crtc = drmModeGetCrtc(fd, crtc_id);
		CHECK(crtc && crtc->mode_valid && crtc->buffer_id == foreign.fb);
		drmModeFreeCrtc(crtc);

		req = drmModeAtomicAlloc();
		CHECK(req);
		property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "FB_ID", foreign.fb);
		for (unsigned int i = 0; i < sizeof(flags) / sizeof(flags[0]); i++) {
			uint32_t expected = flags[i] & DRM_MODE_ATOMIC_TEST_ONLY ?
					    a.fb : foreign.fb;

			CHECK(drmModeSetCrtc(fd, crtc_id, a.fb, 0, 0, &connector_id, 1, mode) == 0);
			CHECK(drmModeAtomicCommit(fd, req, flags[i], NULL) == 0);
			crtc = drmModeGetCrtc(fd, crtc_id);
			CHECK(crtc && crtc->mode_valid && crtc->buffer_id == expected);
			drmModeFreeCrtc(crtc);
		}
		drmModeAtomicFree(req);
		req = drmModeAtomicAlloc();
		CHECK(req);
		property(fd, req, crtc_id, DRM_MODE_OBJECT_CRTC, "ACTIVE", 0);
		property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "FB_ID", foreign.fb);
		CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_ALLOW_MODESET, NULL) == 0);
		drmModeAtomicFree(req);
		req = drmModeAtomicAlloc();
		CHECK(req);
		property(fd, req, crtc_id, DRM_MODE_OBJECT_CRTC, "ACTIVE", 1);
		for (unsigned int i = 0; i < sizeof(flags) / sizeof(flags[0]); i++) {
			CHECK(drmModeAtomicCommit(fd, req, flags[i] | DRM_MODE_ATOMIC_ALLOW_MODESET,
						 NULL) == 0);
		}
		property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "FB_ID", a.fb);
		CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_ALLOW_MODESET, NULL) == 0);
		drmModeAtomicFree(req);
		destroy_buffer(fd, &foreign);
	}
	req = drmModeAtomicAlloc();
	CHECK(req);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "FB_ID", b.fb);
	CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_TEST_ONLY, NULL) == 0);
	drmModeAtomicFree(req);
	crtc = drmModeGetCrtc(fd, crtc_id);
	CHECK(crtc && crtc->mode_valid && crtc->buffer_id == a.fb);
	drmModeFreeCrtc(crtc);
	req = drmModeAtomicAlloc();
	CHECK(req);
	property(fd, req, crtc_id, DRM_MODE_OBJECT_CRTC, "ACTIVE", 0);
	CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_ALLOW_MODESET, NULL) == 0);
	drmModeAtomicFree(req);
	req = drmModeAtomicAlloc();
	CHECK(req);
	property(fd, req, crtc_id, DRM_MODE_OBJECT_CRTC, "ACTIVE", 1);
	CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_ALLOW_MODESET, NULL) == 0);
	drmModeAtomicFree(req);
	check_vblank(fd, mode);
	for (unsigned int i = 0; i < 16; i++) {
		flip(fd, plane_id, b.fb);
		flip(fd, plane_id, b.fb);
		flip(fd, plane_id, a.fb);
	}
	slow_mode = *mode;
	slow_mode.clock /= 2;
	CHECK(slow_mode.clock > 0);
	CHECK(drmModeCreatePropertyBlob(fd, &slow_mode, sizeof(slow_mode), &slow_mode_id) == 0);
	req = drmModeAtomicAlloc();
	CHECK(req);
	property(fd, req, crtc_id, DRM_MODE_OBJECT_CRTC, "MODE_ID", slow_mode_id);
	CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_ALLOW_MODESET, NULL) == 0);
	drmModeAtomicFree(req);
	check_vblank(fd, &slow_mode);
	flip(fd, plane_id, b.fb);
	flip(fd, plane_id, a.fb);
	req = drmModeAtomicAlloc();
	CHECK(req);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_W", mode->hdisplay / 2);
	CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_TEST_ONLY, NULL) == 0);
	/* Scaling is valid; reading beyond the framebuffer is not. */
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "SRC_W",
		 (uint64_t)(mode->hdisplay + 1) << 16);
	errno = 0;
	CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_TEST_ONLY, NULL) != 0 &&
	      errno == ENOSPC);
	drmModeAtomicFree(req);
	req = drmModeAtomicAlloc();
	CHECK(req);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "FB_ID", 0);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_ID", 0);
	property(fd, req, connector_id, DRM_MODE_OBJECT_CONNECTOR, "CRTC_ID", 0);
	property(fd, req, crtc_id, DRM_MODE_OBJECT_CRTC, "ACTIVE", 0);
	property(fd, req, crtc_id, DRM_MODE_OBJECT_CRTC, "MODE_ID", 0);
	CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_ALLOW_MODESET, NULL) == 0);
	drmModeAtomicFree(req);
	crtc = drmModeGetCrtc(fd, crtc_id);
	CHECK(crtc && !crtc->mode_valid);
	drmModeFreeCrtc(crtc);
	destroy_buffer(fd, &b);
	destroy_buffer(fd, &a);
	CHECK(drmModeDestroyPropertyBlob(fd, mode_id) == 0);
	CHECK(drmModeDestroyPropertyBlob(fd, slow_mode_id) == 0);
	drmModeFreeConnector(connector);
	drmModeFreeResources(resources);
	CHECK(close(fd) == 0);
	puts("PASS: CastKMS allocation, modeset, timed vblank, 50 flips, rejection, disable");
	if (argc == 3)
		puts("PASS: HOST accepts retained CPU-mappable PRIME scanout");
	return 0;
}
