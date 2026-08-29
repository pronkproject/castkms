// SPDX-License-Identifier: GPL-2.0-only

#include <linux/dma-buf.h>
#include <linux/err.h>
#include <linux/errno.h>

#include <drm/drm_device.h>
#include <drm/drm_gem.h>
#include <drm/drm_gem_shmem_helper.h>
#include <drm/drm_prime.h>

#include <kunit/visibility.h>

#include "castkms_gem.h"

/**
 * castkms_gem_prime_import() - import a CastKMS GEM object through PRIME
 * @dev: CastKMS DRM device
 * @dma_buf: DMA-BUF to import
 *
 * CastKMS composes frames in software and therefore needs persistent kernel
 * mappings of every scanout buffer. DMA-BUF has no side-effect-free query for
 * that capability, so accepting a foreign object can defer an unsupported
 * vmap operation until atomic commit. Reject it here instead, allowing a
 * multi-GPU compositor to select its normal CPU-copy fallback.
 *
 * A DMA-BUF exported by this same device still refers to a native shmem GEM
 * object and is safe to re-import.
 *
 * Returns: the same-device GEM object, or %ERR_PTR(-EOPNOTSUPP) for a foreign
 * DMA-BUF.
 */
struct drm_gem_object *
castkms_gem_prime_import(struct drm_device *dev, struct dma_buf *dma_buf)
{
	if (!drm_gem_is_prime_exported_dma_buf(dev, dma_buf))
		return ERR_PTR(-EOPNOTSUPP);

	return drm_gem_shmem_prime_import_no_map(dev, dma_buf);
}
EXPORT_SYMBOL_IF_KUNIT(castkms_gem_prime_import);
