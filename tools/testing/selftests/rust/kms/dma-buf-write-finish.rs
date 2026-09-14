// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: borrow of moved value: `write`
#![no_std]

use kernel::{
    dma_buf::cpu_access::Write,
    prelude::*, //
};

pub fn finish(mut write: Write<'_>) -> Result {
    write.copy_from_slice(0, &[1])?;
    write.finish()?;
    #[cfg(negative)]
    write.copy_from_slice(0, &[2])?;
    Ok(())
}
