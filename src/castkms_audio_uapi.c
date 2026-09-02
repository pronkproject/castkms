// SPDX-License-Identifier: GPL-2.0+

#include <linux/build_bug.h>
#include <linux/file.h>
#include <linux/fs.h>

#include <drm/castkms_drm.h>
#include <drm/drm_connector.h>
#include <drm/drm_file.h>

#include "castkms_audio.h"
#include "castkms_audio_uapi.h"
#include "castkms_capture_authority.h"
#include "castkms_connector.h"
#include "castkms_device.h"
#include "castkms_grant.h"

static_assert(sizeof(struct drm_castkms_open_audio_tap) == 48);
static_assert(offsetof(struct drm_castkms_open_audio_tap, buffer_frames) == 32);
static_assert(_IOC_SIZE(DRM_IOCTL_CASTKMS_OPEN_AUDIO_TAP) == 48);

int castkms_audio_open_tap_ioctl(struct drm_device *dev, void *data,
				 struct drm_file *file_priv)
{
	struct drm_castkms_open_audio_tap *args = data;
	struct castkms_device *castkmsdev = drm_device_to_castkms_device(dev);
	struct castkms_capture_authority *authority;
	struct castkms_audio_tap_params params;
	enum castkms_capture_authority_state state;
	struct drm_connector *connector;
	struct file *file;
	int fd;
	int ret;

	if (args->flags || args->fd != -1 ||
	    args->fd_flags & ~O_NONBLOCK || args->format || args->rate ||
	    args->channels || args->frame_bytes || args->buffer_frames ||
	    args->reserved)
		return -EINVAL;
	args->fd = -1;
	if (!castkmsdev->audio)
		return -EOPNOTSUPP;

	connector = drm_connector_lookup(dev, file_priv, args->connector_id);
	if (!connector)
		return -ENOENT;

	mutex_lock(&castkmsdev->attach_transition_lock);
	ret = castkms_grant_begin(
		file_priv, connector,
		CASTKMS_CAPTURE_AUTHORITY_CAPTURE_AUDIO, &authority);
	if (ret)
		goto out_unlock_transition;
	ret = castkms_connector_require_authority_attached(connector,
							 authority);
	if (ret)
		goto out_end_grant;
	ret = castkms_capture_authority_get_state(authority, &state);
	if (ret)
		goto out_end_grant;
	if (state == CASTKMS_CAPTURE_AUTHORITY_SUSPENDED_FOREIGN_CONTENT) {
		ret = castkms_capture_authority_state_status(authority, state);
		goto out_end_grant;
	}

	fd = get_unused_fd_flags(O_CLOEXEC);
	if (fd < 0) {
		ret = fd;
		goto out_end_grant;
	}
	file = castkms_audio_open_tap(castkmsdev->audio, connector, authority,
				      args->fd_flags, &params);
	if (IS_ERR(file)) {
		ret = PTR_ERR(file);
		put_unused_fd(fd);
		goto out_end_grant;
	}

	args->fd = fd;
	args->format = DRM_CASTKMS_AUDIO_FORMAT_S16_LE;
	args->rate = params.rate;
	args->channels = params.channels;
	args->frame_bytes = params.frame_bytes;
	args->buffer_frames = params.buffer_frames;
	fd_install(fd, file);
	ret = 0;

out_end_grant:
	castkms_grant_end(authority);
out_unlock_transition:
	mutex_unlock(&castkmsdev->attach_transition_lock);
	drm_connector_put(connector);
	return ret;
}
