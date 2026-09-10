// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "recycle.h"

int gpu_reuse_main(int argc, char **argv);

/* These doubles check pool policy, not Vulkan synchronization or pixels. */
static struct {
	struct gpu_recycle_image *image;
	unsigned int color;
	int occupied;
	int alive;
} images[5];
static struct gpu_device *devices[3];
static unsigned int image_count, device_count, live_devices, live_images;
static unsigned int calls, fail_at, produced, copied, checked, returned;
static int failed;

static void require(int condition)
{
	if (!condition) {
		fprintf(stderr, "Pool policy assertion failed: call=%u fail_at=%u\n", calls, fail_at);
		exit(1);
	}
}

static int step(void)
{
	/* Error propagation must reach teardown without another operation. */
	require(!failed);
	failed = ++calls == fail_at;
	return failed ? -1 : 0;
}

static unsigned int index_of(struct gpu_recycle_image *image)
{
	unsigned int i;

	for (i = 0; i < image_count; i++)
		if (images[i].image == image && images[i].alive)
			return i;
	require(0);
	return 0;
}

int gpu_context_open(struct gpu_context *context, const char *node, int validation)
{
	(void)node;
	(void)validation;
	memset(context, 0, sizeof(*context));
	atomic_init(&context->validation_errors, 0);
	return step();
}

void gpu_context_close(struct gpu_context *context)
{
	(void)context;
	require(!live_images && !live_devices);
}

int gpu_device_open(struct gpu_device *device, struct gpu_context *context)
{
	(void)context;
	require(device_count < 3);
	/* Include partially constructed devices in the teardown obligation. */
	devices[device_count++] = device;
	live_devices++;
	return step();
}

int gpu_device_close(struct gpu_device *device)
{
	unsigned int i;

	require(!live_images);
	for (i = 0; i < device_count; i++) {
		if (devices[i] == device) {
			devices[i] = NULL;
			live_devices--;
			break;
		}
	}
	return 0;
}

int gpu_recycle_create(struct gpu_recycle_image *image, struct gpu_device *writer,
		       struct gpu_device *reader, uint64_t modifier)
{
	(void)modifier;
	require(image_count < 5 && live_devices == 3);
	require(writer == devices[image_count < 2 ? 0 : 1]);
	require(reader == devices[image_count < 2 ? 1 : 2]);
	images[image_count].image = image;
	images[image_count++].alive = 1;
	live_images++;
	return step();
}

void gpu_recycle_destroy(struct gpu_recycle_image *image)
{
	unsigned int i;

	for (i = 0; i < image_count; i++) {
		if (images[i].image == image) {
			require(images[i].alive);
			images[i].alive = 0;
			live_images--;
			break;
		}
	}
}

int gpu_recycle_source(struct gpu_recycle_image *image, unsigned int color, uint64_t modifier)
{
	unsigned int i = index_of(image);

	(void)modifier;
	require(i < 2 && !images[i].occupied);
	if (step())
		return -1;
	images[i].occupied = 1;
	images[i].color = color;
	produced++;
	return 0;
}

int gpu_recycle_copy(struct gpu_recycle_image *source, struct gpu_recycle_image *destination)
{
	unsigned int e = index_of(source), d = index_of(destination);

	require(e < 2 && d >= 2 && images[e].occupied && !images[d].occupied);
	if (step())
		return -1;
	images[d].color = images[e].color;
	images[e].occupied = 0;
	images[d].occupied = 1;
	copied++;
	return 0;
}

int gpu_recycle_check(struct gpu_recycle_image *image, unsigned int color)
{
	unsigned int i = index_of(image);

	require(i >= 2 && images[i].occupied && images[i].color == color);
	if (step())
		return -1;
	checked++;
	return 0;
}

int gpu_recycle_return(struct gpu_recycle_image *image)
{
	unsigned int i = index_of(image);

	require(i >= 2 && images[i].occupied);
	if (step())
		return -1;
	images[i].occupied = 0;
	returned++;
	return 0;
}

static void reset(unsigned int failure)
{
	memset(images, 0, sizeof(images));
	memset(devices, 0, sizeof(devices));
	image_count = device_count = live_devices = live_images = 0;
	calls = produced = copied = checked = returned = 0;
	failed = 0;
	fail_at = failure;
}

int main(void)
{
	char *argv[] = { "reuse", "/unused/render-node", NULL };
	char *invalid[][7] = {
		{ "reuse", NULL },
		{ "reuse", "/unused", "--unknown", NULL },
		{ "reuse", "/unused", "--validation", "--validation", NULL },
		{ "reuse", "/unused", "--modifier", NULL },
		{ "reuse", "/unused", "--modifier", "", NULL },
		{ "reuse", "/unused", "--modifier", "-1", NULL },
		{ "reuse", "/unused", "--modifier", "+1", NULL },
		{ "reuse", "/unused", "--modifier", " 1", NULL },
		{ "reuse", "/unused", "--modifier", "0x", NULL },
		{ "reuse", "/unused", "--modifier", "1junk", NULL },
		{ "reuse", "/unused", "--modifier", "18446744073709551616", NULL },
		{ "reuse", "/unused", "--modifier", "0", "--modifier", "0", NULL },
	};
	unsigned int i, count, argument_cases = sizeof(invalid) / sizeof(invalid[0]);

	reset(0);
	require(gpu_reuse_main(2, argv) == 0);
	require(produced == 20 && copied == 20 && checked == 40 && returned == 20);
	require(!live_images && !live_devices);
	count = calls;
	for (i = 1; i <= count; i++) {
		reset(i);
		require(gpu_reuse_main(2, argv) == 1);
		require(failed && !live_images && !live_devices);
	}
	printf("PASS: pool policy plus %u injected operation failures\n", count);
	for (i = 0; i < argument_cases; i++) {
		int argc = 0;

		while (invalid[i][argc])
			argc++;
		reset(0);
		require(gpu_reuse_main(argc, invalid[i]) == 1 && !calls);
	}
	printf("PASS: %u argument errors rejected before device discovery\n", argument_cases);
	return 0;
}
