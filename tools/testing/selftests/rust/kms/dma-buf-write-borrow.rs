// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: cannot move out of `buffer` because it is borrowed
#![no_std]

use kernel::{
    dma_buf::{
        cpu_access::Write,
        DmaBuf, //
    },
    prelude::*,
    sync::aref::ARef, //
};

pub fn write(buffer: ARef<DmaBuf>) -> Result {
    let mut access = Write::new(&buffer)?;
    #[cfg(negative)]
    drop(buffer);
    access.copy_from_slice(0, &[1])?;
    access.finish()
}
