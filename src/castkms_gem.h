/* SPDX-License-Identifier: GPL-2.0-only */

#ifndef _CASTKMS_GEM_H_
#define _CASTKMS_GEM_H_

struct dma_buf;
struct drm_device;
struct drm_gem_object;

struct drm_gem_object *
castkms_gem_prime_import(struct drm_device *dev, struct dma_buf *dma_buf);

#endif /* _CASTKMS_GEM_H_ */
