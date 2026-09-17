// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/mman.h>
#include <linux/slab.h>
#include <linux/uaccess.h>
#include <drm/drm_constraints.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_constraints_list.h>
#include <drm/drm_constraints_query.h>
#include <drm/drm_fourcc.h>
#include <uapi/drm/drm_constraints.h>
#include <kunit/test.h>

static void put_list(void *data) { drm_constraints_list_put(data); }
static void put_domain(void *data) { drm_constraints_domain_put(data); }
static void put_description(void *data) { drm_constraints_description_put(data); }
static void put_entry(void *data) { drm_constraints_entry_put(data); }

static struct drm_constraints_list *new_list(struct kunit *test)
{
	const struct drm_constraints_size size = { 640, 480, 640, 480 };
	const struct drm_constraints_format format = {
		.plane_id = 7, .format = DRM_FORMAT_XRGB8888, .size = size,
		.flags = DRM_CONSTRAINTS_FORMAT_IMPLICIT,
		.storage_flags = DRM_CONSTRAINTS_FORMAT_STORAGE_NATIVE,
		.pitch_alignment = 1, .offset_alignment = 1, .max_pitch = U32_MAX,
	};
	struct drm_constraints_description *description;
	struct drm_constraints_domain *domain;
	struct drm_constraints_entry *initial;
	struct drm_constraints_list *list;

	domain = drm_constraints_domain_create(4);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, domain);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_domain, domain), 0);
	description = drm_constraints_description_create(&size, &format, 1, NULL, 0, NULL, 0);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, description);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_description, description), 0);
	initial = drm_constraints_entry_create_stateless(domain, 19, description);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, initial);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, initial), 0);
	list = drm_constraints_list_create(domain, initial, 4);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, list);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_list, list), 0);
	return list;
}

static unsigned long user_page(struct kunit *test)
{
	unsigned long address;

	if (!IS_ENABLED(CONFIG_MMU))
		kunit_skip(test, "userspace copy requires MMU");
	address = kunit_vm_mmap(test, NULL, 0, PAGE_SIZE, PROT_READ | PROT_WRITE,
			       MAP_PRIVATE | MAP_ANONYMOUS, 0);
	KUNIT_ASSERT_NE(test, address, 0);
	KUNIT_ASSERT_FALSE(test, IS_ERR_VALUE(address));
	return address;
}

static void discovery_and_fetch_report_one_snapshot(struct kunit *test)
{
	struct drm_constraints_list *list = new_list(test);
	struct drm_mode_list_constraints query = { .crtc_id = 19 };
	struct drm_mode_constraints_list header;
	unsigned long address = user_page(test);
	u8 *bytes = kunit_kmalloc(test, PAGE_SIZE, GFP_KERNEL);
	u32 required;

	KUNIT_ASSERT_NOT_NULL(test, bytes);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_copy_to_user(list, &query), 0);
	KUNIT_ASSERT_GT(test, query.size, (u32)sizeof(header));
	KUNIT_ASSERT_LE(test, query.size, (u32)PAGE_SIZE);
	KUNIT_EXPECT_NE(test, query.generation, 0);
	required = query.size;
	memset(bytes, 0xa5, PAGE_SIZE);
	KUNIT_ASSERT_EQ(test, copy_to_user((void __user *)address, bytes, PAGE_SIZE), 0);
	query.data = address;
	query.size = U32_MAX;
	KUNIT_ASSERT_EQ(test, drm_constraints_list_copy_to_user(list, &query), 0);
	KUNIT_EXPECT_EQ(test, query.size, required);
	KUNIT_ASSERT_EQ(test, copy_from_user(bytes, (void __user *)address, PAGE_SIZE), 0);
	memcpy(&header, bytes, sizeof(header));
	KUNIT_EXPECT_EQ(test, header.version, DRM_MODE_CONSTRAINTS_VERSION);
	KUNIT_EXPECT_EQ(test, header.length, required);
	KUNIT_EXPECT_EQ(test, header.generation, query.generation);
	KUNIT_EXPECT_EQ(test, header.count_entries, 1);
	KUNIT_EXPECT_EQ(test, header.pad, 0);
	KUNIT_EXPECT_EQ(test, header.reserved[0], 0);
	KUNIT_EXPECT_EQ(test, header.reserved[1], 0);
	for (u32 i = required; i < PAGE_SIZE; i++)
		KUNIT_EXPECT_EQ(test, bytes[i], 0xa5);
}

static void short_buffer_reports_metadata_without_payload(struct kunit *test)
{
	struct drm_constraints_list *list = new_list(test);
	struct drm_mode_list_constraints query = { .crtc_id = 19 };
	unsigned long address = user_page(test);
	u8 bytes[64], observed[64];
	u32 required;

	KUNIT_ASSERT_EQ(test, drm_constraints_list_copy_to_user(list, &query), 0);
	required = query.size;
	memset(bytes, 0xa5, sizeof(bytes));
	KUNIT_ASSERT_EQ(test, copy_to_user((void __user *)address, bytes, sizeof(bytes)), 0);
	query.data = address;
	query.size = sizeof(bytes);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_copy_to_user(list, &query), -ENOSPC);
	KUNIT_EXPECT_EQ(test, query.size, required);
	KUNIT_EXPECT_NE(test, query.generation, 0);
	KUNIT_ASSERT_EQ(test,
			copy_from_user(observed, (void __user *)address, sizeof(observed)), 0);
	KUNIT_EXPECT_MEMEQ(test, bytes, observed, sizeof(bytes));
}

static void stale_closed_and_faulted_queries_preserve_request(struct kunit *test)
{
	struct drm_constraints_list *list = new_list(test);
	struct drm_mode_list_constraints query = { .crtc_id = 19 }, before;

	if (!IS_ENABLED(CONFIG_MMU))
		kunit_skip(test, "userspace fault handling requires MMU");
	KUNIT_ASSERT_EQ(test, drm_constraints_list_copy_to_user(list, &query), 0);
	query.data = 1;
	before = query;
	KUNIT_EXPECT_EQ(test, drm_constraints_list_copy_to_user(list, &query), -EFAULT);
	KUNIT_EXPECT_MEMEQ(test, &query, &before, sizeof(query));
	query.generation++;
	before = query;
	KUNIT_EXPECT_EQ(test, drm_constraints_list_copy_to_user(list, &query), -ESTALE);
	KUNIT_EXPECT_MEMEQ(test, &query, &before, sizeof(query));
	query.generation = 0;
	before = query;
	drm_constraints_list_close(list);
	KUNIT_EXPECT_EQ(test, drm_constraints_list_copy_to_user(list, &query), -ESTALE);
	KUNIT_EXPECT_MEMEQ(test, &query, &before, sizeof(query));
}

static void malformed_requests_preserve_metadata(struct kunit *test)
{
	struct drm_constraints_list *list = new_list(test);
	struct drm_mode_list_constraints cases[] = {
		{ .crtc_id = 0 }, { .crtc_id = 20 },
		{ .crtc_id = 19, .flags = 1 }, { .crtc_id = 19, .pad = 1 },
		{ .crtc_id = 19, .reserved = { 1, 0 } },
		{ .crtc_id = 19, .reserved = { 0, 1 } },
		{ .crtc_id = 19, .data = 1 }, { .crtc_id = 19, .size = 1 },
	};

	for (unsigned int i = 0; i < ARRAY_SIZE(cases); i++) {
		struct drm_mode_list_constraints query = cases[i];

		KUNIT_EXPECT_EQ(test, drm_constraints_list_copy_to_user(list, &query), -EINVAL);
		KUNIT_EXPECT_MEMEQ(test, &query, &cases[i], sizeof(query));
	}
}

static struct kunit_case constraints_query_cases[] = {
	KUNIT_CASE(discovery_and_fetch_report_one_snapshot),
	KUNIT_CASE(short_buffer_reports_metadata_without_payload),
	KUNIT_CASE(stale_closed_and_faulted_queries_preserve_request),
	KUNIT_CASE(malformed_requests_preserve_metadata),
	{}
};

static struct kunit_suite constraints_query_suite = {
	.name = "drm_constraints_query",
	.test_cases = constraints_query_cases,
};
kunit_test_suite(constraints_query_suite);

MODULE_LICENSE("GPL and additional rights");
