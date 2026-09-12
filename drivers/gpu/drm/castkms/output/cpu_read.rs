// SPDX-License-Identifier: GPL-2.0-only

//! Synchronous CPU reads of the current publication, without capture authorization.

use super::*;
use kernel::drm::preparation::ReadClaim;

struct CpuClaim(Option<ReadClaim>);

impl Drop for CpuClaim {
    fn drop(&mut self) {
        if let Some(claim) = self.0.take() {
            claim.release_cpu();
        }
    }
}

impl<S: Clone + Unpin> Output<S> {
    /// Prepare retained mapping resources before admitting a synchronous source read.
    ///
    /// `prepare` may allocate and map storage, but must not read source pixels. `read` runs
    /// only after a claim is acquired and the published generation is revalidated. Both
    /// callbacks run outside the publication lock. Destination storage must be available
    /// before calling; neither callback may wait for downstream reuse.
    /// All source access must finish within `read`; it must not enqueue GPU reads.
    /// The read claim is released before destroying prepared resources, so unmapping may
    /// acquire reservation locks without keeping source retirement pending.
    #[expect(dead_code)]
    pub(crate) fn with_prepared_cpu_scene<P, R>(
        &self,
        prepare: impl FnOnce(&S) -> Result<P>,
        read: impl FnOnce(&S, &P) -> R,
    ) -> Result<Option<R>> {
        let candidate = {
            let state = self.state.lock();
            match &*state {
                Publication::Open(Some(current)) => current
                    .scene
                    .as_ref()
                    .map(|scene| (current.source.clone(), scene.clone())),
                _ => None,
            }
        };
        let Some((source, scene)) = candidate else {
            return Ok(None);
        };
        let resources = prepare(&scene)?;
        let claim = CpuClaim(Some(source.claim()?));
        let current = {
            let state = self.state.lock();
            matches!(&*state, Publication::Open(Some(current))
                if core::ptr::eq(&*current.source, &*source))
        };
        if !current {
            return Err(EAGAIN);
        }
        let result = read(&scene, &resources);
        drop(claim);
        Ok(Some(result))
    }
}
