pub mod tracer;
pub mod typed_obj;
pub mod workers;
pub mod wp;

use anyhow::Result;
use libc::c_void;

fn wrap_libc_call<T: PartialEq>(f: &dyn Fn() -> T, expect: T) -> Result<()> {
    let ret = f();
    if ret == expect {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

pub fn mmap_fixed(start: u64, size: usize, prot: libc::c_int, flags: libc::c_int) -> Result<()> {
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

pub fn mmap_anon(size: usize) -> Result<*mut c_void> {
    let ret = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_ANON | libc::MAP_PRIVATE | libc::MAP_NORESERVE,
            -1,
            0,
        )
    };
    if ret == libc::MAP_FAILED {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(ret)
    }
}

pub const fn align_up(val: usize, align: usize) -> usize {
    val.wrapping_add(align).wrapping_sub(1) & !align.wrapping_sub(1)
}

// pub const fn align_down(val: usize, align: usize) -> usize {
//     val & !align.wrapping_sub(1)
// }
