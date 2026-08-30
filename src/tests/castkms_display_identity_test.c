// SPDX-License-Identifier: GPL-2.0+

#include <kunit/test.h>

#include <linux/string.h>

#include <drm/drm_edid.h>

#include "../castkms_display_identity.h"

MODULE_IMPORT_NS("EXPORTED_FOR_KUNIT_TESTING");

#define TEST_DISPLAYID_OFFSET		(2 * EDID_LENGTH)
#define TEST_PRODUCT_NAME_OFFSET	(TEST_DISPLAYID_OFFSET + 20)

static void castkms_display_identity_make_product_edid(
	struct kunit *test, u8 *edid, size_t edid_size,
	u8 displayid_version, u8 product_tag, const char *product_name)
{
	u8 *displayid = edid + TEST_DISPLAYID_OFFSET;
	size_t product_name_size = strlen(product_name);
	size_t payload_size = 12 + product_name_size;

	KUNIT_ASSERT_GE_MSG(test, edid_size,
			    3 * (size_t)EDID_LENGTH,
			    "fixture needs base, CTA, and DisplayID blocks");
	KUNIT_ASSERT_LE_MSG(test, payload_size + 3,
			    (size_t)121,
			    "product block must fit one DisplayID section");

	edid[126] = 2;
	displayid[0] = 0x70;
	displayid[1] = displayid_version;
	displayid[2] = 3 + payload_size;
	displayid[3] = 4;
	displayid[5] = product_tag;
	displayid[6] = 0;
	displayid[7] = payload_size;
	displayid[8] = 'T';
	displayid[9] = 'O';
	displayid[10] = 'L';
	displayid[19] = product_name_size;
	memcpy(edid + TEST_PRODUCT_NAME_OFFSET, product_name,
	       product_name_size);
}

static void castkms_display_identity_reads_displayid_product(struct kunit *test)
{
	u8 edid[3 * EDID_LENGTH] = {};
	char name[80];

	castkms_display_identity_make_product_edid(
		test, edid, sizeof(edid), 0x13, 0x00,
		"TCL 55A3G Designer Series Smart TV");

	KUNIT_EXPECT_TRUE(test,
		castkms_display_identity_product_name_from_raw(
			edid, sizeof(edid), name, sizeof(name)));
	KUNIT_EXPECT_STREQ(test, name,
			   "TCL 55A3G Designer Series Smart TV");
}

static void castkms_display_identity_reads_displayid_v2_product(
	struct kunit *test)
{
	u8 edid[3 * EDID_LENGTH] = {};
	char name[80];

	castkms_display_identity_make_product_edid(
		test, edid, sizeof(edid), 0x20, 0x20,
		"Living Room Display");

	KUNIT_EXPECT_TRUE(test,
		castkms_display_identity_product_name_from_raw(
			edid, sizeof(edid), name, sizeof(name)));
	KUNIT_EXPECT_STREQ(test, name, "Living Room Display");
}

static void castkms_display_identity_truncates_to_destination(struct kunit *test)
{
	u8 edid[3 * EDID_LENGTH] = {};
	char product_name[107];
	char name[16];

	memset(product_name, 'X', sizeof(product_name) - 1);
	product_name[sizeof(product_name) - 1] = '\0';
	castkms_display_identity_make_product_edid(
		test, edid, sizeof(edid), 0x13, 0x00, product_name);

	KUNIT_EXPECT_TRUE(test,
		castkms_display_identity_product_name_from_raw(
			edid, sizeof(edid), name, sizeof(name)));
	KUNIT_EXPECT_EQ(test, strlen(name), sizeof(name) - 1);
	KUNIT_EXPECT_EQ(test, name[sizeof(name) - 1], '\0');
	KUNIT_EXPECT_EQ(test, strncmp(name, product_name, sizeof(name) - 1), 0);
}

static void castkms_display_identity_ignores_base_name(struct kunit *test)
{
	u8 edid[EDID_LENGTH] = {};
	char name[80] = "stale";

	edid[54 + 3] = 0xfc;
	memcpy(&edid[54 + 5], "Legacy Name", sizeof("Legacy Name") - 1);

	KUNIT_EXPECT_FALSE(test,
		castkms_display_identity_product_name_from_raw(
			edid, sizeof(edid), name, sizeof(name)));
	KUNIT_EXPECT_STREQ(test, name, "");
}

static void castkms_display_identity_rejects_invalid_name(struct kunit *test)
{
	u8 edid[3 * EDID_LENGTH] = {};
	char name[80] = "stale";

	castkms_display_identity_make_product_edid(
		test, edid, sizeof(edid), 0x13, 0x00, "Living Room TV");
	edid[TEST_PRODUCT_NAME_OFFSET + 6] = '\n';

	KUNIT_EXPECT_FALSE(test,
		castkms_display_identity_product_name_from_raw(
			edid, sizeof(edid), name, sizeof(name)));
	KUNIT_EXPECT_STREQ(test, name, "");
}

static void castkms_display_identity_bounds_extensions(struct kunit *test)
{
	u8 edid[EDID_LENGTH] = {};
	char name[80] = "stale";

	edid[126] = 0xff;

	KUNIT_EXPECT_FALSE(test,
		castkms_display_identity_product_name_from_raw(
			edid, sizeof(edid), name, sizeof(name)));
	KUNIT_EXPECT_STREQ(test, name, "");
}

static struct kunit_case castkms_display_identity_cases[] = {
	KUNIT_CASE(castkms_display_identity_reads_displayid_product),
	KUNIT_CASE(castkms_display_identity_reads_displayid_v2_product),
	KUNIT_CASE(castkms_display_identity_truncates_to_destination),
	KUNIT_CASE(castkms_display_identity_ignores_base_name),
	KUNIT_CASE(castkms_display_identity_rejects_invalid_name),
	KUNIT_CASE(castkms_display_identity_bounds_extensions),
	{}
};

static struct kunit_suite castkms_display_identity_suite = {
	.name = "castkms-display-identity",
	.test_cases = castkms_display_identity_cases,
};

kunit_test_suite(castkms_display_identity_suite);

MODULE_LICENSE("GPL");
