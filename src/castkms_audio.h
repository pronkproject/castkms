/* SPDX-License-Identifier: GPL-2.0+ */

#ifndef _CASTKMS_AUDIO_H_
#define _CASTKMS_AUDIO_H_

struct castkms_device;
struct castkms_audio;
struct drm_connector;

#ifdef CASTKMS_HAVE_AUDIO

int castkms_audio_init(struct castkms_device *castkmsdev);
void castkms_audio_cleanup(struct castkms_device *castkmsdev);
void castkms_audio_notify_eld(struct castkms_device *castkmsdev,
			      struct drm_connector *connector);
void castkms_audio_notify_disconnect(struct castkms_device *castkmsdev,
				     struct drm_connector *connector);

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
			 struct drm_connector *connector) { }

static inline void
castkms_audio_notify_disconnect(struct castkms_device *castkmsdev,
				struct drm_connector *connector) { }

#endif /* CASTKMS_HAVE_AUDIO */
#endif /* _CASTKMS_AUDIO_H_ */
