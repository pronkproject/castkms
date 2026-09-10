// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <errno.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "recycle.h"

/* Unequal fixture budgets, not kernel limits or transport-window requirements. */
#define STAGING_COUNT 2
#define OUTPUT_COUNT 3
#define ROUNDS 4

struct slot {
	struct gpu_recycle_image image;
	int occupied;
	unsigned int color;
};

struct pools {
	struct slot staging[STAGING_COUNT];
	struct slot output[OUTPUT_COUNT];
	unsigned int retired;
	unsigned int delivered;
	uint64_t modifier;
};

/* Backpressure is positive; negative results abort the experiment. */
static int capture(struct pools *pools, unsigned int color)
{
	unsigned int i;

	for (i = 0; i < STAGING_COUNT; i++) {
		struct slot *slot = &pools->staging[i];

		if (slot->occupied)
			continue;
		/* No source exists until independently available staging has been selected. */
		if (gpu_recycle_source(&slot->image, color, pools->modifier))
			return -1;
		slot->occupied = 1;
		slot->color = color;
		pools->retired++;
		return 0;
	}
	return 1;
}

static int deliver(struct pools *pools)
{
	unsigned int e, d;

	for (e = 0; e < STAGING_COUNT; e++) {
		struct slot *staging = &pools->staging[e];

		if (!staging->occupied)
			continue;
		for (d = 0; d < OUTPUT_COUNT; d++) {
			struct slot *output = &pools->output[d];

			if (output->occupied)
				continue;
			if (gpu_recycle_copy(&staging->image, &output->image))
				return -1;
			output->occupied = 1;
			output->color = staging->color;
			staging->occupied = 0;
			pools->delivered++;
			return gpu_recycle_check(&output->image, output->color);
		}
		return 1;
	}
	return 1;
}

static int check_held(struct pools *pools)
{
	unsigned int i;

	for (i = 0; i < OUTPUT_COUNT; i++)
		if (pools->output[i].occupied &&
		    gpu_recycle_check(&pools->output[i].image, pools->output[i].color))
			return -1;
	return 0;
}

static int return_outputs(struct pools *pools)
{
	unsigned int i;

	for (i = 0; i < OUTPUT_COUNT; i++) {
		struct slot *slot = &pools->output[i];

		if (!slot->occupied)
			continue;
		if (gpu_recycle_return(&slot->image))
			return -1;
		slot->occupied = 0;
	}
	return 0;
}

static int exercise(struct pools *pools)
{
	unsigned int round, i, color = 0;

	for (round = 0; round < ROUNDS; round++) {
		unsigned int retired;

		for (i = 0; i < OUTPUT_COUNT; i++)
			if (capture(pools, color++) || deliver(pools))
				return -1;
		retired = pools->retired;
		for (i = 0; i < STAGING_COUNT; i++)
			if (capture(pools, color++) || deliver(pools) != 1)
				return -1;
		if (pools->retired != retired + STAGING_COUNT || capture(pools, color) != 1 ||
		    pools->retired != retired + STAGING_COUNT || check_held(pools))
			return -1;
		printf("round=%u held_outputs=%u sources_retired_while_held=%u source_unbound=1\n",
		       round, OUTPUT_COUNT, pools->retired - retired);
		if (return_outputs(pools))
			return -1;
		for (i = 0; i < STAGING_COUNT; i++)
			if (deliver(pools))
				return -1;
		if (check_held(pools) || return_outputs(pools))
			return -1;
	}
	printf("retired=%u delivered=%u staging_allocations=%u output_allocations=%u\n",
	       pools->retired, pools->delivered, STAGING_COUNT, OUTPUT_COUNT);
	return pools->retired == ROUNDS * (STAGING_COUNT + OUTPUT_COUNT) &&
	       pools->retired == pools->delivered ? 0 : -1;
}

static int reuse(struct gpu_context *context, uint64_t modifier)
{
	struct gpu_device source = { 0 }, output = { 0 }, consumer = { 0 };
	struct pools pools = { .modifier = modifier };
	unsigned int i;
	int result = 1;

	if (gpu_device_open(&source, context) || gpu_device_open(&output, context) ||
	    gpu_device_open(&consumer, context))
		goto out;
	for (i = 0; i < STAGING_COUNT; i++)
		if (gpu_recycle_create(&pools.staging[i].image, &source, &output, modifier))
			goto out;
	for (i = 0; i < OUTPUT_COUNT; i++)
		if (gpu_recycle_create(&pools.output[i].image, &output, &consumer, modifier))
			goto out;
	result = exercise(&pools) ? 1 : 0;
out:
	/* Each operation drains accepted commands, including its failure path. */
	for (i = 0; i < STAGING_COUNT; i++)
		gpu_recycle_destroy(&pools.staging[i].image);
	for (i = 0; i < OUTPUT_COUNT; i++)
		gpu_recycle_destroy(&pools.output[i].image);
	if (gpu_device_close(&consumer))
		result = 1;
	if (gpu_device_close(&output))
		result = 1;
	if (gpu_device_close(&source))
		result = 1;
	return result;
}

int main(int argc, char **argv)
{
	struct gpu_context context;
	uint64_t modifier = 0;
	int validation = 0, modifier_set = 0, i, result;

	if (argc < 2)
		goto usage;
	for (i = 2; i < argc; i++) {
		if (!strcmp(argv[i], "--validation") && !validation) {
			validation = 1;
		} else if (!strcmp(argv[i], "--modifier") && !modifier_set && i + 1 < argc) {
			unsigned long long value;
			char *end;

			i++;
			if (argv[i][0] < '0' || argv[i][0] > '9')
				goto usage;
			errno = 0;
			value = strtoull(argv[i], &end, 0);
			if (errno || end == argv[i] || *end || value > UINT64_MAX)
				goto usage;
			modifier = value;
			modifier_set = 1;
		} else {
			goto usage;
		}
	}
	result = gpu_context_open(&context, argv[1], validation);
	if (result)
		return result == -ENODEV ? 4 : 1;
	printf("reuse_modifier=0x%016" PRIx64 " image_size=256x256\n", modifier);
	result = reuse(&context, modifier);
	gpu_context_close(&context);
	if (atomic_load(&context.validation_errors))
		result = 1;
	if (!result)
		puts("PASS: persistent staging and output pools preserve held pixels across reuse");
	return result;
usage:
	fprintf(stderr, "Usage: %s RENDER_NODE [--validation] [--modifier INTEGER]\n", argv[0]);
	return 1;
}
