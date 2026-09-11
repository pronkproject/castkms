# SPDX-License-Identifier: GPL-2.0

obj-$(CONFIG_DRM_KUNIT_TEST_HELPERS) += \
	drm_kunit_helpers.o

obj-$(CONFIG_DRM_KUNIT_TEST) += \
	drm_atomic_test.o \
	drm_atomic_blob_test.o \
	drm_atomic_request_test.o \
	drm_atomic_request_apply_test.o \
	drm_atomic_user_value_test.o \
	drm_atomic_user_request_test.o \
	drm_atomic_user_request_auth_test.o \
	drm_atomic_user_signaling_test.o \
	drm_atomic_user_fence_test.o \
	drm_atomic_user_commit_test.o \
	drm_atomic_geometry_test.o \
	drm_atomic_rotation_test.o \
	drm_atomic_plane_color_test.o \
	drm_atomic_cursor_test.o \
	drm_atomic_power_test.o \
	drm_atomic_gamma_test.o \
	drm_atomic_color_test.o \
	drm_atomic_fence_test.o \
	drm_atomic_prepare_test.o \
	drm_atomic_prepare_set_test.o \
	drm_atomic_prepare_outputs_test.o \
	drm_atomic_prepare_display_test.o \
	drm_atomic_prepare_request_test.o \
	drm_atomic_prepare_request_owner_test.o \
	drm_atomic_prepare_setcrtc_test.o \
	drm_atomic_prepare_setplane_test.o \
	drm_atomic_prepare_cursor_test.o \
	drm_atomic_prepare_power_test.o \
	drm_atomic_prepare_remove_test.o \
	drm_atomic_prepare_ticket_outputs_test.o \
	drm_atomic_prepare_guard_test.o \
	drm_atomic_prepare_ticket_test.o \
	drm_atomic_prepare_owner_test.o \
	drm_atomic_prepare_auth_test.o \
	drm_atomic_prepare_commit_test.o \
	drm_atomic_prepare_file_test.o \
	drm_atomic_commit_test.o \
	drm_atomic_swap_test.o \
	drm_bridge_test.o \
	drm_capture_test.o \
	drm_capture_wait_test.o \
	drm_capture_authority_test.o \
	drm_capture_file_test.o \
	drm_cmdline_parser_test.o \
	drm_connector_test.o \
	drm_crtc_test.o \
	drm_damage_helper_test.o \
	drm_dp_mst_helper_test.o \
	drm_exec_test.o \
	drm_format_helper_test.o \
	drm_format_test.o \
	drm_framebuffer_test.o \
	drm_gem_shmem_test.o \
	drm_hdmi_state_helper_test.o \
	drm_managed_test.o \
	drm_mm_test.o \
	drm_modes_test.o \
	drm_plane_helper_test.o \
	drm_probe_helper_test.o \
	drm_rect_test.o \
	drm_sysfb_modeset_test.o \
	drm_fixp_test.o \
	drm_kunit_edid.o

ifeq ($(CONFIG_DRM_CLIENT),y)
obj-$(CONFIG_DRM_KUNIT_TEST) += drm_atomic_prepare_client_test.o
endif

CFLAGS_drm_mm_test.o := $(DISABLE_STRUCTLEAK_PLUGIN)
