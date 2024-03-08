use anyhow::Result;
use wp::{Object, Slot};

use crate::ObjectModel;

fn wrap_libc_call<T: PartialEq>(f: &dyn Fn() -> T, expect: T) -> Result<()> {
    let ret = f();
    if ret == expect {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

fn mmap_fixed(start: u64, size: usize, prot: libc::c_int, flags: libc::c_int) -> Result<()> {
    let ptr = start as *mut libc::c_void;
    wrap_libc_call(
        &|| unsafe { libc::mmap(ptr, size, prot, flags, -1, 0) },
        ptr,
    )?;
    Ok(())
}

pub fn munmap(start: u64, size: usize) -> Result<()> {
    let ptr = start as *mut libc::c_void;
    wrap_libc_call(&|| unsafe { libc::munmap(ptr, size) }, 0)
}

pub fn dzmmap_noreplace(start: u64, size: usize) -> Result<()> {
    let prot = libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC;
    let flags =
        libc::MAP_ANON | libc::MAP_PRIVATE | libc::MAP_FIXED_NOREPLACE | libc::MAP_NORESERVE;

    mmap_fixed(start, size, prot, flags)
}

pub trait ObjectOps {
    fn get(&self) -> u64;
    fn scan_object<O: ObjectModel, F: FnMut(Slot)>(&self, mut f: F) {
        O::scan_object(self.get(), |edge, repeat| {
            for i in 0..repeat {
                let ptr = edge.wrapping_add(i as usize);
                f(Slot(ptr));
            }
        })
    }
    fn mark(&self, mark_state: u8) -> bool {
        crate::trace::trace_object_atomic(self.get(), mark_state)
    }
}

impl ObjectOps for Object {
    fn get(&self) -> u64 {
        self.0
    }
}

pub mod workers;
