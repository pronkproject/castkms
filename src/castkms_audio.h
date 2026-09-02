/* SPDX-License-Identifier: GPL-2.0+ */

#ifndef _CASTKMS_AUDIO_H_
#define _CASTKMS_AUDIO_H_

#include <linux/err.h>
#include <linux/errno.h>
#include <linux/types.h>

struct castkms_device;
struct castkms_audio;
struct castkms_capture_authority;
struct drm_connector;
struct drm_edid;
struct file;

struct castkms_audio_tap_params {
	u32 rate;
	u32 channels;
	u32 frame_bytes;
	u64 buffer_frames;
};

#ifdef CASTKMS_HAVE_AUDIO

int castkms_audio_init(struct castkms_device *castkmsdev);
void castkms_audio_cleanup(struct castkms_device *castkmsdev);
void castkms_audio_notify_eld(struct castkms_device *castkmsdev,
			      struct drm_connector *connector,
			      const struct drm_edid *drm_edid,
			      const char *display_name);
void castkms_audio_notify_disconnect(struct castkms_device *castkmsdev,
				     struct drm_connector *connector);
struct file *
castkms_audio_open_tap(struct castkms_audio *audio,
		       struct drm_connector *connector,
		       struct castkms_capture_authority *authority,
		       u32 fd_flags, struct castkms_audio_tap_params *params);

#else /* !CASTKMS_HAVE_AUDIO */

static inline int
castkms_audio_init(struct castkms_device *castkmsdev)
{
	return 0;
}

static inline void
castkms_audio_cleanup(struct castkms_device *castkmsdev) { }

static inline void
castkms_audio_notify_eld(struct castkms_device *castkmsdev,
			 struct drm_connector *connector,
			 const struct drm_edid *drm_edid,
			 const char *display_name) { }

static inline void
castkms_audio_notify_disconnect(struct castkms_device *castkmsdev,
				struct drm_connector *connector) { }

static inline struct file *
castkms_audio_open_tap(struct castkms_audio *audio,
		       struct drm_connector *connector,
		       struct castkms_capture_authority *authority,
		       u32 fd_flags, struct castkms_audio_tap_params *params)
{
	return ERR_PTR(-EOPNOTSUPP);
}

#endif /* CASTKMS_HAVE_AUDIO */
#endif /* _CASTKMS_AUDIO_H_ */
