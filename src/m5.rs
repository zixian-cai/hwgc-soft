use libc::{c_char, c_uint, c_void};

#[link(name = "m5")]
extern "C" {
    pub fn m5_arm(address: u64);
    pub fn m5_quiesce();
    pub fn m5_quiesce_ns(ns: u64);
    pub fn m5_quiesce_cycle(cycles: u64);
    pub fn m5_quiesce_time() -> u64;
    pub fn m5_rpns() -> u64;
    pub fn m5_wake_cpu(cpuid: u64);

    pub fn m5_exit(ns_delay: u64);
    pub fn m5_fail(ns_delay: u64, code: u64);
    pub fn m5_sum(a: c_uint, b: c_uint, c: c_uint, d: c_uint, e: c_uint, f: c_uint) -> c_uint;
    pub fn m5_init_param(key_str1: u64, key_str2: u64) -> u64;
    pub fn m5_checkpoint(ns_delay: u64, ns_period: u64);
    pub fn m5_reset_stats(ns_delay: u64, ns_period: u64);
    pub fn m5_dump_stats(ns_delay: u64, ns_period: u64);
    pub fn m5_dump_reset_stats(ns_delay: u64, ns_period: u64);
    pub fn m5_read_file(buffer: *mut c_void, len: u64, offset: u64) -> u64;
    pub fn m5_write_file(buffer: *mut c_void, len: u64, offset: u64, filename: *const c_char);
    pub fn m5_debug_break();
    pub fn m5_switch_cpu();
    pub fn m5_dist_toggle_sync();
    pub fn m5_add_symbol(addr: u64, symbol: *const c_char);
    pub fn m5_load_symbol();
    pub fn m5_panic();
    pub fn m5_work_begin(workid: u64, threadid: u64);
    pub fn m5_work_end(workid: u64, threadid: u64);
}
