/* SPDX-License-Identifier: GPL-2.0-only */

#ifndef _CASTKMS_AUDIO_UAPI_H_
#define _CASTKMS_AUDIO_UAPI_H_

struct drm_device;
struct drm_file;

int castkms_audio_open_tap_ioctl(struct drm_device *dev, void *data,
				 struct drm_file *file_priv);

#endif /* _CASTKMS_AUDIO_UAPI_H_ */
