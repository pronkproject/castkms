/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef CASTKMS_AUDIO_FIXTURE_H
#define CASTKMS_AUDIO_FIXTURE_H

#include "fixture.h"
#include "../../../../include/uapi/drm/castkms_drm.h"

void audio_edid(unsigned char bytes[256]);
int audio_card(unsigned int output);
struct drm_castkms_audio_files audio_capture(int fd, uint32_t crtc, uint32_t connector);
void audio_terminal(int fd);

#endif
