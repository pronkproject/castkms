// SPDX-License-Identifier: GPL-2.0+

#include <linux/minmax.h>
#include <linux/string.h>

#include <drm/drm_edid.h>

#include <kunit/visibility.h>

#include "castkms_display_identity.h"

#define CASTKMS_DISPLAYID_EXTENSION_TAG		0x70
#define CASTKMS_DISPLAYID_V1_PRODUCT_ID_TAG	0x00
#define CASTKMS_DISPLAYID_V2_PRODUCT_ID_TAG	0x20
#define CASTKMS_DISPLAYID_HEADER_SIZE		5
#define CASTKMS_DISPLAYID_BLOCK_HEADER_SIZE	3
#define CASTKMS_DISPLAYID_PRODUCT_FIXED_SIZE	12

static bool castkms_display_identity_copy_product_name(const u8 *payload,
							 size_t payload_size,
							 char *name,
							 size_t name_size)
{
	size_t product_name_size;
	size_t copy_size;
	size_t i;

	if (payload_size < CASTKMS_DISPLAYID_PRODUCT_FIXED_SIZE || !name_size)
		return false;

	product_name_size = payload[11];
	if (!product_name_size ||
	    product_name_size >
		payload_size - CASTKMS_DISPLAYID_PRODUCT_FIXED_SIZE)
		return false;

	for (i = 0; i < product_name_size; i++) {
		u8 byte = payload[CASTKMS_DISPLAYID_PRODUCT_FIXED_SIZE + i];

		if (byte < 0x20 || byte > 0x7e)
			return false;
	}

	copy_size = min(product_name_size, name_size - 1);
	memcpy(name, payload + CASTKMS_DISPLAYID_PRODUCT_FIXED_SIZE,
	       copy_size);
	name[copy_size] = '\0';

	return true;
}

static bool castkms_display_identity_product_name_from_extension(
	const u8 *extension, char *name, size_t name_size)
{
	size_t section_end;
	size_t offset;

	if (extension[0] != CASTKMS_DISPLAYID_EXTENSION_TAG)
		return false;

	section_end = CASTKMS_DISPLAYID_HEADER_SIZE + extension[2];
	/* Byte 126 is DisplayID's structure checksum; byte 127 is EDID's. */
	if (section_end > EDID_LENGTH - 2)
		return false;

	for (offset = CASTKMS_DISPLAYID_HEADER_SIZE;
	     offset < section_end;) {
		const u8 *block = extension + offset;
		size_t payload_size;
		size_t block_size;

		if (section_end - offset < CASTKMS_DISPLAYID_BLOCK_HEADER_SIZE)
			return false;

		payload_size = block[2];
		block_size = CASTKMS_DISPLAYID_BLOCK_HEADER_SIZE + payload_size;
		if (block_size > section_end - offset)
			return false;

		if ((block[0] == CASTKMS_DISPLAYID_V1_PRODUCT_ID_TAG ||
		     block[0] == CASTKMS_DISPLAYID_V2_PRODUCT_ID_TAG) &&
		    castkms_display_identity_copy_product_name(
			    block + CASTKMS_DISPLAYID_BLOCK_HEADER_SIZE,
			    payload_size, name, name_size))
			return true;

		offset += block_size;
	}

	return false;
}

VISIBLE_IF_KUNIT bool castkms_display_identity_product_name_from_raw(
	const u8 *edid, size_t edid_size, char *name, size_t name_size)
{
	size_t block_count;
	size_t block;

	if (!edid || edid_size < EDID_LENGTH || !name || !name_size)
		return false;

	name[0] = '\0';
	block_count = min_t(size_t, (size_t)edid[126] + 1,
			    edid_size / EDID_LENGTH);
	for (block = 1; block < block_count; block++) {
		if (castkms_display_identity_product_name_from_extension(
			    edid + block * EDID_LENGTH, name, name_size))
			return true;
	}

	return false;
}
EXPORT_SYMBOL_IF_KUNIT(castkms_display_identity_product_name_from_raw);

bool castkms_display_identity_product_name(const struct drm_edid *drm_edid,
					   char *name, size_t name_size)
{
	const struct edid *raw;
	size_t raw_size;

	if (!drm_edid || !name || !name_size)
		return false;

	raw = drm_edid_raw(drm_edid);
	if (!raw) {
		name[0] = '\0';
		return false;
	}

	/* drm_edid_raw() guarantees that this advertised extent was allocated. */
	raw_size = ((size_t)raw->extensions + 1) * EDID_LENGTH;
	return castkms_display_identity_product_name_from_raw(
		(const u8 *)raw, raw_size, name, name_size);
}
